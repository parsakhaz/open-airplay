"""open-airplay sender sidecar: casts from this PC to AirPlay devices (Apple TV, HomePod, AirPlay speakers).

Protocol: one JSON object per line on stdin (commands) and stdout (events). Every command runs as its
own task, so a long cast never blocks a scan or another device. Replies carry the command's id.
  -> {"id": 1, "cmd": "scan", "timeout": 5}
  <- {"id": 1, "event": "devices", "devices": [{"identifier", "name", "address", "model", "services"}]}
  -> {"id": 2, "cmd": "play_url", "identifier": "...", "name": "Den", "url": "http://..." | "C:/movie.mp4"}
  -> {"id": 3, "cmd": "stream_file", "identifier": "...", "name": "Den", "path": "C:/song.mp3"}
  <- {"id": 2, "event": "playing", "identifier": "..."}           the device took it (not final)
  <- {"id": 2, "event": "ok" | "stopped" | "error", "identifier": "..."}   final: ended, stopped, failed
  -> {"id": 4, "cmd": "stop", "identifier": "..."}   stops that device's cast (no identifier: all casts)
  -> {"id": 5, "cmd": "pause" | "resume", "identifier": "..."}
  -> {"id": 6, "cmd": "ping"}     <- {"id": 6, "event": "pong"}
  -> {"id": 7, "cmd": "health"}   <- {"id": 7, "event": "health", "pyatv", "frozen", "casting": [identifiers]}
Errors are {"event": "error", "code": "<code>", "message": "<a plain sentence>"}; technical detail
goes to stderr, which the app writes to sender.log.
The first line is always {"event": "ready", "pyatv": "<version>"}.
Built on pyatv (MIT).
"""

import asyncio
import json
import logging
import socket
import sys

import pyatv
from pyatv import exceptions
from pyatv.const import Protocol

try:
    from importlib.metadata import version as _pkg_version

    PYATV_VERSION = _pkg_version("pyatv")
except Exception:  # pragma: no cover
    PYATV_VERSION = "unknown"

log = logging.getLogger("oap_sender")

# Longest a scan may run, whatever the request asks for.
MAX_SCAN_SECONDS = 8
# Finding and connecting to one device; the whole attempt ends with a plain error after this.
CONNECT_SECONDS = 12
# After connecting, how long a cast may fail before it counts as playing.
START_GRACE_SECONDS = 2
# Closing a device's connection when a cast stops.
CLOSE_SECONDS = 1.5


class Refused(Exception):
    """A failure with a plain message, already decided."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


def plain_error(exc, name, media="that"):
    """Maps an exception to (code, plain sentence). Never shows library or exception names."""
    if isinstance(exc, Refused):
        return exc.code, exc.message
    if isinstance(exc, (FileNotFoundError, IsADirectoryError, PermissionError)):
        return "bad_file", "Couldn't open that file. Check it still exists."
    if isinstance(exc, (exceptions.AuthenticationError, exceptions.NoCredentialsError,
                        exceptions.InvalidCredentialsError, exceptions.PairingError)):
        return "needs_pairing", f"{name} needs to be paired with this PC first."
    if isinstance(exc, (exceptions.NotSupportedError, exceptions.NoServiceError,
                        exceptions.UnsupportedProtocolError)):
        return "unsupported", f"{name} can't play {media}."
    if isinstance(exc, exceptions.ConnectionLostError):
        return "lost", f"Lost the connection to {name}."
    if isinstance(exc, exceptions.BlockedStateError):
        return "busy", f"{name} is busy with something else. Try again in a moment."
    if isinstance(exc, (exceptions.PlaybackError, exceptions.HttpError, exceptions.UnknownMediaKindError)):
        return "playback", f"{name} couldn't play {media}."
    if isinstance(exc, (asyncio.TimeoutError, TimeoutError, LookupError, OSError,
                        exceptions.ConnectionFailedError, exceptions.OperationTimeoutError,
                        exceptions.NonLocalSubnetError)):
        return "unreachable", f"{name} didn't answer. Check it's on and on the same network."
    return "failed", f"Couldn't cast to {name}. Try again."


def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def describe(conf):
    model = None
    try:
        model = conf.device_info.model_str
    except Exception:
        pass
    return {
        "identifier": conf.identifier,
        "name": conf.name,
        "address": str(conf.address),
        "model": model,
        "services": sorted(s.protocol.name for s in conf.services),
    }


def local_addresses():
    """This PC's own addresses, so its own receiver is not offered as a place to cast to."""
    addrs = {"127.0.0.1", "::1"}
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None):
            addrs.add(info[4][0].split("%")[0])
    except OSError:
        pass
    try:  # the address of the route out, which getaddrinfo can miss; sends nothing
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("192.0.2.1", 9))
            addrs.add(s.getsockname()[0])
    except OSError:
        pass
    return addrs


class Job:
    def __init__(self, request_id, identifier):
        self.request_id = request_id
        self.identifier = identifier
        self.task = None
        self.atv = None
        self.stopping = False


class Sidecar:
    def __init__(self, emit=emit, scan=None, connect=None, local=local_addresses):
        self.emit = emit
        self.scan_fn = scan or (lambda **kw: pyatv.scan(asyncio.get_running_loop(), **kw))
        self.connect_fn = connect or (lambda conf: pyatv.connect(conf, asyncio.get_running_loop()))
        self.local = local
        self.jobs = {}  # identifier -> Job: at most one cast per device
        self.known = {}  # identifier -> config from the last scan, so a cast needn't scan again
        self.tasks = set()

    def dispatch(self, cmd):
        """Runs one command as its own task and returns it."""
        task = asyncio.ensure_future(self.run(cmd))
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)
        return task

    async def run(self, cmd):
        rid = cmd.get("id")
        try:
            reply = await self.handle(cmd)
        except Exception as e:  # report every failure to the shell; never crash the sidecar
            code, message = plain_error(e, cmd.get("name") or "The device")
            log.warning("command %s (%s) failed: %r", rid, cmd.get("cmd"), e)
            reply = {"event": "error", "code": code, "message": message}
        if reply is not None:
            self.emit({"id": rid, **reply})

    async def handle(self, cmd):
        kind = cmd.get("cmd")
        if kind == "ping":
            return {"event": "pong"}
        if kind == "health":
            return {"event": "health", "pyatv": PYATV_VERSION, "frozen": bool(getattr(sys, "frozen", False)),
                    "casting": sorted(self.jobs)}
        if kind == "scan":
            return await self.scan(cmd)
        if kind in ("play_url", "stream_file"):
            await self.cast(cmd)
            return None  # the cast reports for itself
        if kind == "stop":
            return await self.stop(cmd.get("identifier"))
        if kind in ("pause", "resume"):
            return await self.pause(cmd, kind == "pause")
        raise Refused("bad_request", f"unknown command: {kind}")

    async def scan(self, cmd):
        try:
            seconds = max(1, min(int(cmd.get("timeout", 5)), MAX_SCAN_SECONDS))
        except (TypeError, ValueError):
            seconds = 5
        try:
            confs = await asyncio.wait_for(self.scan_fn(timeout=seconds), seconds + 2)
        except Exception as e:
            log.warning("scan failed: %r", e)
            raise Refused("scan_failed", "Couldn't search the network. Check this PC is on Wi-Fi or a network.")
        mine = self.local() if cmd.get("exclude_self", True) else set()
        found = [c for c in confs if str(c.address) not in mine]
        self.known.update((c.identifier, c) for c in found)
        return {"event": "devices", "devices": [describe(c) for c in found]}

    async def cast(self, cmd):
        identifier = cmd.get("identifier")
        if not identifier:
            raise Refused("bad_request", "Choose a device first.")
        play_url = cmd["cmd"] == "play_url"
        media = cmd.get("url" if play_url else "path")
        if not media:
            raise Refused("bad_request", "Add a link or choose a file first.")
        old = self.jobs.get(identifier)
        if old:
            await self.end(old)
        job = Job(cmd.get("id"), identifier)
        self.jobs[identifier] = job
        job.task = asyncio.current_task()
        name = cmd.get("name") or "The device"
        what = "that link" if play_url and "://" in media else "that file"

        def reply(event, **extra):
            self.emit({"id": job.request_id, "event": event, "identifier": identifier, **extra})

        try:
            job.atv = await asyncio.wait_for(self.open(identifier, cmd.get("credentials")), CONNECT_SECONDS)
            stream = job.atv.stream
            playing = asyncio.ensure_future(stream.play_url(media) if play_url else stream.stream_file(media))
            try:
                done, _ = await asyncio.wait({playing}, timeout=START_GRACE_SECONDS)
                if not done:
                    reply("playing")
                await playing
            finally:
                if not playing.done():
                    playing.cancel()
            reply("ok")
        except asyncio.CancelledError:
            if job.stopping:
                reply("stopped")
                return
            raise
        except Exception as e:
            code, message = plain_error(e, name, what)
            log.warning("cast %s to %s failed: %r", job.request_id, identifier, e)
            reply("error", code=code, message=message)
        finally:
            await self.close(job)
            if self.jobs.get(identifier) is job:
                del self.jobs[identifier]

    async def open(self, identifier, credentials):
        conf = self.known.get(identifier)
        if conf is None:
            confs = await self.scan_fn(identifier=identifier, timeout=5)
            if not confs:
                raise LookupError(f"device {identifier} not found")
            conf = self.known[identifier] = confs[0]
        for proto, creds in (credentials or {}).items():
            conf.set_credentials(Protocol[proto], creds)
        return await self.connect_fn(conf)

    async def close(self, job):
        atv, job.atv = job.atv, None
        if atv is None:
            return
        try:
            tasks = atv.close()
            if tasks:
                await asyncio.wait_for(asyncio.gather(*tasks, return_exceptions=True), CLOSE_SECONDS)
        except Exception as e:
            log.info("closing %s: %r", job.identifier, e)

    async def end(self, job):
        """Cancels a cast and waits for it to report "stopped" and close its connection."""
        job.stopping = True
        if job.task and not job.task.done():
            job.task.cancel()
            await asyncio.wait({job.task}, timeout=CLOSE_SECONDS + 1)

    async def stop(self, identifier):
        jobs = [self.jobs[identifier]] if identifier in self.jobs else [] if identifier else list(self.jobs.values())
        await asyncio.gather(*(self.end(j) for j in jobs))
        return {"event": "ok", "stopped": [j.identifier for j in jobs]}

    async def pause(self, cmd, pause):
        job = self.jobs.get(cmd.get("identifier"))
        name = cmd.get("name") or "The device"
        if job is None or job.atv is None:
            raise Refused("not_casting", f"Nothing is playing on {name}.")
        rc = job.atv.remote_control
        await asyncio.wait_for(rc.pause() if pause else rc.play(), CONNECT_SECONDS)
        return {"event": "ok"}

    async def shutdown(self):
        await self.stop(None)


async def main():
    logging.basicConfig(stream=sys.stderr, level=logging.INFO,
                        format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    loop = asyncio.get_running_loop()
    sidecar = Sidecar()
    emit({"event": "ready", "pyatv": PYATV_VERSION})
    log.info("ready (pyatv %s, frozen=%s)", PYATV_VERSION, bool(getattr(sys, "frozen", False)))
    while True:
        line = await loop.run_in_executor(None, sys.stdin.readline)
        if not line:
            break
        if not line.strip():
            continue
        try:
            cmd = json.loads(line)
        except json.JSONDecodeError as e:
            emit({"event": "error", "code": "bad_request", "message": f"bad json: {e}"})
            continue
        if not isinstance(cmd, dict):
            emit({"event": "error", "code": "bad_request", "message": "each command must be a JSON object"})
            continue
        log.info("command %s: %s %s", cmd.get("id"), cmd.get("cmd"), cmd.get("identifier") or "")
        sidecar.dispatch(cmd)
    # stdin closed: the app is gone or stopped us. Stop every cast and let replies in flight finish.
    await sidecar.shutdown()
    if sidecar.tasks:
        await asyncio.wait(set(sidecar.tasks), timeout=CLOSE_SECONDS + 1)


if __name__ == "__main__":
    asyncio.run(main())
