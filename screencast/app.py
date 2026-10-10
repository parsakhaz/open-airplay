"""The screencast app: one aiohttp server that

* serves the live HLS stream to the LAN at /hls/ (the Apple TV pulls from here), and
* serves the control UI and /api/ to this PC only (loopback).
"""
from __future__ import annotations

import asyncio
import json
import re
import os
import shutil
import subprocess
import tempfile
import time
from dataclasses import asdict, replace
from pathlib import Path

from aiohttp import web

from . import caps, firewall, receiver
from .negotiate import LATENCY, apply_overrides, negotiate
from .pipeline import MASTER, ffmpeg_command

UI = Path(__file__).with_name("ui")
CACHE = Path(os.environ.get("LOCALAPPDATA", tempfile.gettempdir())) / "open-airplay" / "screencast-caps.json"
MIME = {".m3u8": "application/vnd.apple.mpegurl", ".ts": "video/mp2t", ".m4s": "video/iso.segment",
        ".mp4": "video/mp4"}


def _ffmpeg_version():
    try:
        return caps._run(["-version"]).stdout.splitlines()[0]
    except Exception:
        return ""


def load_machine(reprobe=False) -> caps.MachineCaps:
    """Displays are read fresh every time; encoder probing (~25 s) is cached per ffmpeg build and CPU."""
    key = {"ffmpeg": _ffmpeg_version(), "cpus": os.cpu_count()}
    encoders = None
    if not reprobe and CACHE.exists():
        try:
            data = json.loads(CACHE.read_text())
            if data.get("key") == key:
                encoders = tuple(caps.Encoder(**e) for e in data["encoders"])
        except Exception:
            pass
    if encoders is None:
        encoders = caps.list_encoders()
        CACHE.parent.mkdir(parents=True, exist_ok=True)
        CACHE.write_text(json.dumps({"key": key, "encoders": [asdict(e) for e in encoders]}))
    audio = caps.list_audio_sources()
    notes = () if audio else ('No system-audio loopback device found (enable "Stereo Mix" or install VB-CABLE): '
                              "casting video only.",)
    return caps.MachineCaps(displays=caps.list_displays(), encoders=encoders, audio_sources=audio,
                            cpu_count=os.cpu_count() or 1, notes=notes)


_SEGMENT = re.compile(rb"^seg_(\d+)\.(m4s|ts)$", re.M)


def advertise_ahead(playlist: bytes, count: int, segment: float) -> bytes:
    """List the next `count` segments before they exist. AVPlayer starts ~3 target durations behind the
    newest listed segment; when some of those are still being encoded, it starts that much closer to live.
    The server holds requests for them until ffmpeg finishes each one (see App.hls)."""
    # HLS requires an integer target duration >= every segment (so >= 1 s); ffmpeg rounds 0.5 s down to 0,
    # which makes Apple TV reject the whole playlist.
    playlist = re.sub(rb"#EXT-X-TARGETDURATION:0\b", b"#EXT-X-TARGETDURATION:1", playlist)
    found = _SEGMENT.findall(playlist)
    if not count or not found:
        return playlist
    num, ext = found[-1]
    width = len(num)
    extra = b"".join(b"#EXTINF:%.6f,\nseg_%s.%s\n" % (segment, str(int(num) + i).zfill(width).encode(), ext)
                     for i in range(1, count + 1))
    return playlist + extra


def region_for(rect, display: caps.Display):
    """A window's desktop rect -> region inside `display` (clipped), in that display's pixels."""
    l, t, r, b = rect
    l, t = max(l, display.x), max(t, display.y)
    r, b = min(r, display.x + display.width), min(b, display.y + display.height)
    if r - l < 64 or b - t < 64:
        raise ValueError("that window is not on the chosen display")
    return (l - display.x, t - display.y, r - l, b - t)


class Session:
    """ffmpeg -> HLS dir -> Apple TV. Tracks live encoder stats and whether the receiver is pulling."""

    def __init__(self, settings, display, region, rx, port):
        self.settings, self.display, self.region, self.rx, self.port = settings, display, region, rx, port
        self.dir = Path(tempfile.mkdtemp(prefix="oap-screencast-"))
        self.proc: asyncio.subprocess.Process | None = None
        self.cast: receiver.Cast | None = None
        self.stats: dict = {}
        self.stderr: list[str] = []
        self.last_pull: float | None = None
        self.pulls = 0
        self.segment_pulls = 0  # media segments the receiver itself fetched from us
        self.verdict_logged = None
        self.started = time.time()
        self.phase = "starting"
        self.url = None

    async def start(self):
        cmd = ffmpeg_command(self.settings, self.display, self.region)
        self.proc = await asyncio.create_subprocess_exec(
            *cmd, cwd=self.dir, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        from . import win

        win.kill_with_this_process(self.proc.pid)  # no orphaned screen capture if we die
        asyncio.create_task(self._read_progress())
        asyncio.create_task(self._read_stderr())
        # The playlist must already hold a couple of segments, or the receiver may give up on an empty live stream.
        need = 2
        for _ in range(200):
            if self.proc.returncode is not None:
                raise RuntimeError("ffmpeg exited: " + " | ".join(self.stderr[-3:]))
            if len(list(self.dir.glob("seg_*"))) >= need and (self.dir / MASTER).exists():
                break
            await asyncio.sleep(0.1)
        else:
            raise RuntimeError("ffmpeg produced no segments in 20 s")
        self.url = f"http://{receiver.local_address_for(self.rx['address'])}:{self.port}/hls/{MASTER}"
        self.phase = "connecting"
        self.cast = receiver.Cast(self.rx["identifier"], self.url)
        await self.cast.start()
        self.phase = "casting"

    async def _read_progress(self):
        cur = {}
        while self.proc and (line := await self.proc.stdout.readline()):
            k, _, v = line.decode(errors="replace").strip().partition("=")
            cur[k] = v
            if k == "progress":
                self.stats = {"fps": cur.get("fps"), "speed": cur.get("speed"), "frames": cur.get("frame"),
                              "dropped": cur.get("drop_frames"), "dup": cur.get("dup_frames")}

    async def _read_stderr(self):
        while self.proc and (line := await self.proc.stderr.readline()):
            self.stderr = (self.stderr + [line.decode(errors="replace").strip()])[-20:]

    def status(self):
        ffmpeg_alive = self.proc is not None and self.proc.returncode is None
        error = self.cast.error if self.cast else None
        if self.proc is not None and not ffmpeg_alive:
            error = "encoder stopped: " + (" | ".join(self.stderr[-3:]) or f"exit {self.proc.returncode}")
        pulling = self.last_pull is not None and time.time() - self.last_pull < 6
        tv_state = receiver.tvos26.state["playback"]
        real = tv_state == "playing" and self.segment_pulls > 0
        verdict = ("REAL DESKTOP ON TV" if real else
                   "NOT confirmed: receiver has not played our desktop segments yet")
        if verdict != self.verdict_logged:
            print(f"[screencast] {verdict}: {self.url} (receiver={self.rx['name']} state={tv_state} "
                  f"segments_fetched={self.segment_pulls} url={self.url})", flush=True)
            self.verdict_logged = verdict
        hint = None
        if self.phase == "casting" and not error and not pulling and time.time() - self.started > 15:
            hint = ("The receiver is not fetching the stream. Allow Python through Windows Firewall on private "
                    "networks, and make sure this PC and the receiver are on the same LAN.")
        return {"phase": self.phase, "receiver": self.rx["name"], "url": self.url, "settings": asdict(self.settings),
                "stats": self.stats, "receiver_pulling": pulling, "pulls": self.pulls,
                "segments_fetched": self.segment_pulls, "receiver_state": tv_state, "verdict": verdict, "error": error,
                "hint": hint, "uptime": round(time.time() - self.started)}

    async def stop(self):
        if self.cast:
            await self.cast.stop()
        if self.proc and self.proc.returncode is None:
            try:
                self.proc.stdin and self.proc.stdin.close()
                self.proc.terminate()
                await asyncio.wait_for(self.proc.wait(), 5)
            except Exception:
                self.proc.kill()
        shutil.rmtree(self.dir, ignore_errors=True)


class App:
    def __init__(self, port: int, reprobe: bool):
        self.port, self.reprobe = port, reprobe
        self.machine: caps.MachineCaps | None = None
        self.receivers: list[dict] = []
        self.session: Session | None = None
        self.pairing: receiver.Pairing | None = None
        self.firewall: dict | None = None
        self.lock = asyncio.Lock()

    # ------------------------------------------------------------------ helpers
    def _refresh_displays(self):
        # Monitors come and go (docks, lids); never act on a stale list.
        self.machine = replace(self.machine, displays=caps.list_displays())

    def _display(self, index):
        self._refresh_displays()
        d = next((d for d in self.machine.displays if d.index == int(index)), None)
        if not d:
            raise ValueError("that display is no longer connected")
        return d

    def _rx(self, identifier):
        rx = next((r for r in self.receivers if r["identifier"] == identifier), None)
        rx = rx or next((r for r in self.receivers if r["name"].lower() == str(identifier).lower()), None)
        if not rx:
            raise web.HTTPBadRequest(text="unknown receiver; scan again")
        return rx

    def _target(self, body):
        display = self._display(body["display"])
        region = None
        if body.get("window"):
            from . import win

            w = next((w for w in win.list_windows() if w["hwnd"] == int(body["window"])), None)
            if not w:
                raise ValueError("that window is gone")
            region = region_for(w["rect"], display)
        elif body.get("region"):
            region = tuple(int(v) for v in body["region"])
        # Negotiate against what is actually captured: the region's size, not the whole display's.
        shape = replace(display, width=region[2], height=region[3]) if region else display
        return display, region, shape

    # ------------------------------------------------------------------ routes
    async def index(self, request):
        return web.FileResponse(UI / "index.html")

    async def state(self, request):
        if self.machine is not None:
            self._refresh_displays()
        m = self.machine
        return web.json_response({
            "machine": None if m is None else {
                "displays": [asdict(d) for d in m.displays], "encoders": [asdict(e) for e in m.encoders],
                "audio": [a.name for a in m.audio_sources], "cpus": m.cpu_count, "notes": list(m.notes)},
            "receivers": [{**{k: v for k, v in r.items() if k != "caps"}, "caps": receiver.caps_json(r["caps"])}
                          for r in self.receivers],
            "session": self.session.status() if self.session else None,
            "setup": {"firewall": self.firewall, "paired": sorted(receiver.load_credentials())},
        })

    async def firewall_check(self, request):
        self.firewall = await asyncio.get_running_loop().run_in_executor(None, firewall.status, self.port)
        return web.json_response(self.firewall)

    async def firewall_allow(self, request):
        ok = firewall.request_allow(self.port)  # Windows shows its own UAC prompt; the user decides
        return web.json_response({"requested": ok})

    async def scan(self, request):
        self.receivers = await receiver.scan()
        return await self.state(request)

    async def windows(self, request):
        from . import win

        return web.json_response([w for w in win.list_windows() if w["title"] != "Program Manager"])

    async def plan(self, request):
        body = await request.json()
        try:
            _, _, shape = self._target(body)
            p = negotiate(self.machine, self._rx(body["receiver"])["caps"], shape, body.get("encoder"))
        except ValueError as e:
            return web.json_response({"error": str(e)}, status=400)
        return web.json_response(p.to_json())

    async def begin(self, body: dict) -> tuple[int, dict]:
        """Start casting; returns (http status, payload). Shared by the UI and the `cast` CLI."""
        async with self.lock:
            if self.session:
                await self.session.stop()
                self.session = None
            try:
                rx = self._rx(body["receiver"])
                display, region, shape = self._target(body)
                p = negotiate(self.machine, rx["caps"], shape, body.get("encoder"))
                settings = apply_overrides(p, self.machine, rx["caps"], shape, body.get("overrides") or {})
            except ValueError as e:
                return 400, {"error": str(e)}
            self.session = Session(settings, display, region, rx, self.port)
            try:
                await self.session.start()
            except receiver.PairingRequired as e:
                await self.session.stop()
                self.session = None
                return 409, {"error": str(e), "pairing_required": True}
            except Exception as e:
                status = self.session.status()
                await self.session.stop()
                self.session = None
                return 500, {"error": str(e) or type(e).__name__, "session": status}
        return 200, self.session.status()

    async def start(self, request):
        code, payload = await self.begin(await request.json())
        return web.json_response(payload, status=code)

    async def pair_begin(self, request):
        body = await request.json()
        if self.pairing:
            await self.pairing.close()
        self.pairing = receiver.Pairing(self._rx(body["receiver"])["identifier"])
        try:
            await self.pairing.begin()
        except Exception as e:
            self.pairing = None
            return web.json_response({"error": str(e) or type(e).__name__}, status=500)
        return web.json_response({"ok": True})

    async def pair_finish(self, request):
        body = await request.json()
        if not self.pairing:
            return web.json_response({"error": "start pairing first"}, status=400)
        try:
            await self.pairing.finish(str(body.get("pin", "")).strip())
        except Exception as e:
            return web.json_response({"error": str(e) or type(e).__name__}, status=400)
        finally:
            self.pairing = None
        return web.json_response({"ok": True})

    async def stop(self, request):
        async with self.lock:
            if self.session:
                await self.session.stop()
                self.session = None
        return await self.state(request)

    async def hls(self, request):
        s = self.session
        name = request.match_info["name"]
        if not s or "/" in name or "\\" in name or ".." in name:
            raise web.HTTPNotFound()
        path = s.dir / name
        lat = LATENCY[s.settings.latency]
        if not path.is_file() and _SEGMENT.match(name.encode()) and lat["ahead"]:
            # An advertised segment still being encoded: hold the request until ffmpeg renames it into place
            # (hls temp_file flag), so the receiver gets it the moment it exists.
            deadline = time.monotonic() + (lat["ahead"] + 2) * lat["segment"] + 2
            while not path.is_file() and time.monotonic() < deadline and self.session is s:
                await asyncio.sleep(0.02)
        if os.environ.get("OAP_DEBUG") and request.remote == s.rx["address"]:
            print(f"[receiver] {request.method} {name} -> {'200' if path.is_file() else '404'} "
                  f"range={request.headers.get('Range')}", flush=True)
        if not path.is_file():
            raise web.HTTPNotFound()
        if request.remote == s.rx["address"]:  # only the receiver's own requests count as proof
            s.pulls += 1
            s.last_pull = time.time()
            if path.suffix in (".m4s", ".ts"):
                s.segment_pulls += 1
        ext = path.suffix
        headers = {"Content-Type": MIME.get(ext, "application/octet-stream"), "Access-Control-Allow-Origin": "*",
                   "Cache-Control": "no-cache" if ext == ".m3u8" else "max-age=60"}
        body = path.read_bytes()
        if name == "stream.m3u8":
            body = advertise_ahead(body, lat["ahead"], lat["segment"])  # also repairs TARGETDURATION
        return web.Response(body=body, headers=headers)

    @web.middleware
    async def loopback_only(self, request, handler):
        if not request.path.startswith("/hls/") and request.remote not in ("127.0.0.1", "::1"):
            raise web.HTTPForbidden(text="the control UI is only available on this PC")
        return await handler(request)

    async def on_startup(self, app):
        loop = asyncio.get_running_loop()
        self.machine = await loop.run_in_executor(None, load_machine, self.reprobe)
        self.firewall = await loop.run_in_executor(None, firewall.status, self.port)
        self.receivers = await receiver.scan()

    async def on_cleanup(self, app):
        if self.session:
            await self.session.stop()

    def build(self) -> web.Application:
        app = web.Application(middlewares=[self.loopback_only])
        app.router.add_get("/", self.index)
        app.router.add_static("/ui/", UI)
        app.router.add_get("/api/state", self.state)
        app.router.add_post("/api/scan", self.scan)
        app.router.add_get("/api/windows", self.windows)
        app.router.add_post("/api/plan", self.plan)
        app.router.add_post("/api/start", self.start)
        app.router.add_post("/api/stop", self.stop)
        app.router.add_post("/api/pair/begin", self.pair_begin)
        app.router.add_post("/api/pair/finish", self.pair_finish)
        app.router.add_post("/api/setup/firewall/check", self.firewall_check)
        app.router.add_post("/api/setup/firewall/allow", self.firewall_allow)
        app.router.add_get("/hls/{name}", self.hls)
        app.on_startup.append(self.on_startup)
        app.on_cleanup.append(self.on_cleanup)
        return app
