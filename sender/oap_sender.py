"""open-airplay sender sidecar: casts from this PC to AirPlay devices (Apple TV, HomePod, AirPlay speakers).

Protocol: one JSON object per line on stdin (commands) and stdout (events). Every command runs as its
own task, so a long cast never blocks a scan or another device. Replies carry the command's id.
  -> {"id": 1, "cmd": "scan", "timeout": 5}                    "hosts": ["192.168.1.50"] asks those addresses directly
  <- {"id": 1, "event": "devices", "devices": [{"identifier", "name", "address", "model", "services"}]}
  -> {"id": 2, "cmd": "play_url", "identifier": "...", "name": "Den", "url": "http://..." | "C:/movie.mp4"}
  -> {"id": 3, "cmd": "stream_file", "identifier": "...", "name": "Den", "path": "C:/song.mp3"}
  <- {"id": 2, "event": "playing", "identifier": "..."}           the device took it (not final)
  <- {"id": 2, "event": "ok" | "stopped" | "error", "identifier": "..."}   final: ended, stopped, failed
  play_url, stream_file, pause and resume also take "address", used to reach a device no scan has seen.
  -> {"id": 4, "cmd": "stop", "identifier": "..."}   stops that device's cast (no identifier: all casts)
  -> {"id": 5, "cmd": "pause" | "resume", "identifier": "..."}
  -> {"id": 6, "cmd": "ping"}     <- {"id": 6, "event": "pong"}
  -> {"id": 7, "cmd": "health"}   <- {"id": 7, "event": "health", "pyatv", "frozen", "casting": [identifiers]}
play_url and stream_file also take "credentials": {"<protocol>": "<credentials>", "password": "..."},
saved by the app from an earlier pairing. A cast that needs them fails with code "needs_pairing".
Pairing, for a device that asks ("protocol" is "AirPlay" for play_url, "RAOP" for stream_file):
  -> {"id": 8, "cmd": "pair_begin", "identifier": "...", "name": "Den", "protocol": "AirPlay", "address"?}
  <- {"id": 8, "event": "pairing", "identifier": "...", "credential": "pin" | "password"}
       "pin": the device now shows a 4-digit code. "password": it has a password set; nothing is shown.
  -> {"id": 9, "cmd": "pair_code", "identifier": "...", "code": "1234"}
  <- {"id": 9, "event": "paired", "identifier": "...", "credentials": {"AirPlay": "..."}}
       or an error with code "wrong_code": the device now shows a new code; send pair_code again.
  -> {"id": 10, "cmd": "pair_cancel", "identifier": "..."}   <- {"id": 10, "event": "ok"}
Credentials and codes are never written to the log.
Errors are {"event": "error", "code": "<code>", "message": "<a plain sentence>"}; technical detail
goes to stderr, which the app writes to sender.log.
The first line is always {"event": "ready", "pyatv": "<version>"}.
Built on pyatv (MIT).
"""

import asyncio
import ipaddress
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


def to_ipv4(host):
    """An IPv4 address for a host (an IPv4 literal passes through), or None. pyatv only scans IPv4."""
    host = str(host).strip()
    try:
        return str(ipaddress.IPv4Address(host))
    except ValueError:
        pass
    try:
        infos = socket.getaddrinfo(host, None, socket.AF_INET)
    except (OSError, UnicodeError):
        return None
    return infos[0][4][0] if infos else None


async def resolve_hosts(hosts):
    """Resolves each host now (a remembered hostname's address can change), dropping ones that don't."""
    loop = asyncio.get_running_loop()
    found = await asyncio.gather(*(loop.run_in_executor(None, to_ipv4, h) for h in hosts if str(h).strip()))
    return list(dict.fromkeys(a for a in found if a))


class Pairing:
    """A pairing in progress with one device: pyatv's handler, or None when the device has a password."""

    def __init__(self, protocol, handler, name):
        self.protocol = protocol
        self.handler = handler
        self.name = name


class Job:
    def __init__(self, request_id, identifier):
        self.request_id = request_id
        self.identifier = identifier
        self.task = None
        self.atv = None
        self.stopping = False


class Sidecar:
    def __init__(self, emit=emit, scan=None, connect=None, local=local_addresses, pair=None):
        self.emit = emit
        self.scan_fn = scan or (lambda **kw: pyatv.scan(asyncio.get_running_loop(), **kw))
        self.connect_fn = connect or (lambda conf: pyatv.connect(conf, asyncio.get_running_loop()))
        self.pair_fn = pair or (lambda conf, protocol: pyatv.pair(conf, protocol, asyncio.get_running_loop()))
        self.local = local
        self.pairings = {}  # identifier -> Pairing in progress
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
        if kind == "pair_begin":
            return await self.pair_begin(cmd)
        if kind == "pair_code":
            return await self.pair_code(cmd)
        if kind == "pair_cancel":
            await self.pair_cancel(cmd.get("identifier"))
            return {"event": "ok"}
        raise Refused("bad_request", f"unknown command: {kind}")

    async def scan(self, cmd):
        try:
            seconds = max(1, min(int(cmd.get("timeout", 5)), MAX_SCAN_SECONDS))
        except (TypeError, ValueError):
            seconds = 5
        asked = cmd.get("hosts") or []
        hosts = await resolve_hosts(asked)
        if asked and not hosts:
            return {"event": "devices", "devices": []}  # none of the addresses resolved
        kwargs = {"hosts": hosts} if hosts else {}  # unicast: for networks that block discovery
        try:
            confs = await asyncio.wait_for(self.scan_fn(timeout=seconds, **kwargs), seconds + 2)
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
            job.atv = await asyncio.wait_for(self.open(identifier, cmd.get("credentials"), cmd.get("address")), CONNECT_SECONDS)
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

    async def find(self, identifier, address=None):
        """The device's config: from the last scan, else asked for by address or by identifier."""
        conf = self.known.get(identifier)
        if conf is None:
            if address:  # added by address: ask it directly, discovery may be blocked
                hosts = await resolve_hosts([address])
                if not hosts:
                    raise LookupError(f"{address} doesn't resolve")
                confs = [c for c in await self.scan_fn(hosts=hosts, timeout=5) if c.identifier == identifier]
            else:
                confs = await self.scan_fn(identifier=identifier, timeout=5)
            if not confs:
                raise LookupError(f"device {identifier} not found")
            conf = self.known[identifier] = confs[0]
        return conf

    async def open(self, identifier, credentials, address=None):
        conf = await self.find(identifier, address)
        for proto, creds in (credentials or {}).items():
            if proto == "password":  # a password set on the device: every service that asks gets it
                for service in conf.services:
                    service.password = creds
            elif proto in Protocol.__members__:
                conf.set_credentials(Protocol[proto], creds)
        return await self.connect_fn(conf)

    async def pair_begin(self, cmd):
        identifier = cmd.get("identifier")
        if not identifier:
            raise Refused("bad_request", "Choose a device first.")
        name = cmd.get("name") or "The device"
        protocol = Protocol.__members__.get(cmd.get("protocol") or "AirPlay")
        if protocol not in (Protocol.AirPlay, Protocol.RAOP):
            raise Refused("bad_request", f"can't pair {cmd.get('protocol')}")
        await self.pair_cancel(identifier)  # a fresh start replaces one left open
        conf = await asyncio.wait_for(self.find(identifier, cmd.get("address")), CONNECT_SECONDS)
        service = conf.get_service(protocol)
        if service is None:
            raise Refused("unsupported", f"{name} can't be paired with this PC.")
        if getattr(service, "requires_password", False):
            # Nothing to show: the password is checked when the cast connects.
            self.pairings[identifier] = Pairing(protocol, None, name)
            return {"event": "pairing", "identifier": identifier, "credential": "password"}
        handler = await self.pair_fn(conf, protocol)
        try:
            await asyncio.wait_for(handler.begin(), CONNECT_SECONDS)
        except BaseException:
            await self.close_pairing(handler)
            raise
        self.pairings[identifier] = Pairing(protocol, handler, name)
        return {"event": "pairing", "identifier": identifier, "credential": "pin"}

    async def pair_code(self, cmd):
        identifier = cmd.get("identifier")
        pairing = self.pairings.get(identifier)
        if pairing is None:
            raise Refused("not_pairing", "That code has expired. Try casting again.")
        code = str(cmd.get("code") or "").strip()
        if not code:
            raise Refused("bad_request", "Enter the code first.")
        if pairing.handler is None:
            del self.pairings[identifier]
            return {"event": "paired", "identifier": identifier, "credentials": {"password": code}}
        handler = pairing.handler
        try:
            handler.pin(code)
            await asyncio.wait_for(handler.finish(), CONNECT_SECONDS)
        except Exception as e:
            log.warning("pairing with %s failed: %s", identifier, type(e).__name__)
            # The device drops a failed attempt; start again so it shows a new code to type.
            await self.pair_cancel(identifier)
            await self.pair_begin({"identifier": identifier, "name": pairing.name, "protocol": pairing.protocol.name})
            raise Refused("wrong_code", f"That code didn't work. Enter the new code shown on {pairing.name}.")
        if not handler.has_paired:
            await self.pair_cancel(identifier)
            raise Refused("failed", f"Couldn't pair with {pairing.name}. Try again.")
        credentials = handler.service.credentials
        await self.pair_cancel(identifier)
        log.info("paired with %s (%s)", identifier, pairing.protocol.name)
        return {"event": "paired", "identifier": identifier, "credentials": {pairing.protocol.name: credentials}}

    async def pair_cancel(self, identifier):
        pairing = self.pairings.pop(identifier, None)
        if pairing and pairing.handler:
            await self.close_pairing(pairing.handler)

    async def close_pairing(self, handler):
        try:
            await asyncio.wait_for(handler.close(), CLOSE_SECONDS)
        except Exception as e:
            log.info("closing a pairing: %s", type(e).__name__)

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
        for identifier in list(self.pairings):
            await self.pair_cancel(identifier)


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
