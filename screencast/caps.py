"""What this machine and the chosen receiver can actually do.

Machine detection is Windows-specific (DXGI/GDI via ctypes, ffmpeg probes); the dataclasses and
receiver parsing are pure so the negotiation logic can be tested anywhere.
"""
from __future__ import annotations

import os
import re
import subprocess
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass

FFMPEG = os.environ.get("OAP_FFMPEG", "ffmpeg")


@dataclass(frozen=True)
class Display:
    index: int  # DXGI output index on the default adapter (what ffmpeg's ddagrab output_idx means)
    device: str  # \\.\DISPLAYn
    name: str
    x: int  # desktop position, physical pixels
    y: int
    width: int  # physical pixels, independent of Windows DPI scaling
    height: int
    refresh: int
    scale: float  # Windows DPI scale (1.0 = 100%)
    primary: bool


@dataclass(frozen=True)
class Encoder:
    name: str  # ffmpeg encoder name
    codec: str  # "h264" | "hevc"
    hardware: bool
    max_height: int  # tallest output it sustains in real time on this machine
    max_fps: int


@dataclass(frozen=True)
class AudioSource:
    name: str  # dshow device that carries system audio (loopback)


@dataclass(frozen=True)
class MachineCaps:
    displays: tuple
    encoders: tuple
    audio_sources: tuple = ()
    cpu_count: int = 1
    notes: tuple = ()


@dataclass(frozen=True)
class ReceiverCaps:
    name: str
    model: str
    video: bool  # can play AirPlay video URLs at all
    hevc: bool
    max_width: int
    max_height: int
    max_fps: int
    hdr: bool
    source: str  # where the caps came from, shown in the UI


# ---------------------------------------------------------------- receiver

# Models known to decode HEVC in AVPlayer HLS. Unknown models get H.264 only: never offer what may fail.
_UHD_HEVC = ("AppleTV6,2", "AppleTV11,1", "AppleTV14,1")  # Apple TV 4K gen 1/2/3


def _features(value) -> int:
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value:
        parts = [int(p, 16) for p in value.split(",")]
        return parts[0] | (parts[1] << 32 if len(parts) > 1 else 0)
    return 0


def parse_receiver(info: dict) -> ReceiverCaps:
    """Build caps from a receiver's GET /info plist (or its mDNS TXT record, which has fewer keys)."""
    model = str(info.get("model", ""))
    video = bool(_features(info.get("features")) & 1)  # AirPlay feature bit 0: Video
    hdr_cap = str(info.get("receiverHDRCapability", "") or "").lower()
    if model in _UHD_HEVC:
        hevc, w, h, fps, source = True, 3840, 2160, 60, f"{model} (Apple TV 4K)"
    elif model.startswith("AppleTV5,3"):
        hevc, w, h, fps, source = False, 1920, 1080, 60, f"{model} (Apple TV HD)"
    elif model.startswith(("Mac", "iMac")):
        hevc, w, h, fps, source = True, 3840, 2160, 60, f"{model} (Mac AirPlay receiver)"
    else:
        hevc, w, h, fps, source = False, 1920, 1080, 30, f"{model or 'unknown model'} (conservative defaults)"
    m = re.match(r"(?:4k|1080p?)(\d+)", hdr_cap)
    if m:  # tvOS advertises e.g. "4k60"; trust it over the model table for fps
        fps = int(m.group(1))
    return ReceiverCaps(
        name=str(info.get("name", "")), model=model, video=video, hevc=hevc,
        max_width=w, max_height=h, max_fps=fps,
        hdr=bool(hdr_cap) and hdr_cap != "none", source=source,
    )


# ---------------------------------------------------------------- machine (Windows)

def _run(args, timeout=20):
    return subprocess.run([FFMPEG, "-hide_banner", *args], capture_output=True, text=True, timeout=timeout,
                          creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))


def list_displays() -> tuple:
    from . import win

    return tuple(win.list_displays())


_LOOPBACK_HINTS = ("stereo mix", "what u hear", "loopback", "virtual-audio-capturer", "cable output",
                   "wave out", "voicemeeter out")


def list_audio_sources() -> tuple:
    out = _run(["-list_devices", "true", "-f", "dshow", "-i", "dummy"]).stderr
    names = re.findall(r'"([^"]+)" \(audio\)', out)
    return tuple(AudioSource(n) for n in names if any(h in n.lower() for h in _LOOPBACK_HINTS))


# hardware first; the probe decides what is real on this machine
_CANDIDATES = (
    ("hevc_nvenc", "hevc", True), ("hevc_amf", "hevc", True), ("hevc_qsv", "hevc", True),
    ("h264_nvenc", "h264", True), ("h264_amf", "h264", True), ("h264_qsv", "h264", True),
    ("libx265", "hevc", False), ("libx264", "h264", False),
)
_SW_FAST = {"libx264": ["-preset", "veryfast", "-tune", "zerolatency"],
            "libx265": ["-preset", "ultrafast", "-tune", "zerolatency", "-x265-params", "log-level=error"]}


def _speed(stderr: str) -> float:
    found = re.findall(r"speed=\s*([\d.]+)x", stderr)
    return float(found[-1]) if found else 0.0


def _encode_test(name, w, h, fps, seconds):
    r = _run(["-loglevel", "error", "-stats", "-f", "lavfi", "-i", f"testsrc2=size={w}x{h}:rate={fps}",
              "-t", str(seconds), "-pix_fmt", "nv12", "-c:v", name, *_SW_FAST.get(name, []), "-f", "null", "-"],
             timeout=60)
    return r.returncode == 0, _speed(r.stderr)


def probe_encoder(name, codec, hardware):
    """Return an Encoder if it works here, sized to what it sustains in real time; None otherwise."""
    if hardware:
        # Fixed-function encoders run 4K60 once they open at 4K; prove they open on this GPU and driver.
        for h in (2160, 1440, 1080):
            ok, _ = _encode_test(name, h * 16 // 9, h, 60, 0.2)
            if ok:
                return Encoder(name, codec, True, h, 60)
        return None
    # Software: benchmark on synthetic motion (harder than a typical desktop, so conservative).
    for h, fps in ((2160, 60), (1440, 60), (1080, 60), (1080, 30), (720, 30)):
        ok, speed = _encode_test(name, h * 16 // 9, h, fps, 2)
        if not ok:
            return None
        if speed >= 1.15:  # headroom for capture and the rest of the system
            return Encoder(name, codec, False, h, fps)
    return None


def list_encoders() -> tuple:
    avail = _run(["-encoders"]).stdout
    cands = [c for c in _CANDIDATES if re.search(rf"\s{c[0]}\s", avail)]
    with ThreadPoolExecutor(max_workers=4) as pool:
        hw = [e for e in pool.map(lambda c: probe_encoder(*c), [c for c in cands if c[2]]) if e]
    # Software benchmarks run one at a time so they don't steal each other's CPU.
    sw = [e for e in (probe_encoder(*c) for c in cands if not c[2]) if e]
    return tuple(hw + sw)


def detect_machine() -> MachineCaps:
    with ThreadPoolExecutor(max_workers=3) as pool:
        d, a, e = pool.submit(list_displays), pool.submit(list_audio_sources), pool.submit(list_encoders)
        displays, audio, encoders = d.result(), a.result(), e.result()
    notes = []
    if not audio:
        notes.append('No system-audio loopback device found (enable "Stereo Mix" or install VB-CABLE): '
                     "casting video only.")
    return MachineCaps(displays=displays, encoders=encoders, audio_sources=audio,
                       cpu_count=os.cpu_count() or 1, notes=tuple(notes))
