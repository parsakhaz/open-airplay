"""open-airplay sender sidecar: casts from this PC to AirPlay devices (Apple TV, HomePod, AirPlay speakers).

Protocol: one JSON object per line on stdin (commands) and stdout (events).
  -> {"id": 1, "cmd": "scan", "timeout": 5}
  <- {"id": 1, "event": "devices", "devices": [{"identifier", "name", "address", "model", "services"}]}
  -> {"id": 2, "cmd": "play_url", "identifier": "...", "url": "http://..."}     video/audio URL (AirPlay)
  -> {"id": 3, "cmd": "stream_file", "identifier": "...", "path": "C:/song.mp3"} audio file (RAOP/AirPlay)
  -> {"id": 4, "cmd": "ping"}
  <- {"id": N, "event": "ok" | "pong" | "error", ...}
The first line is always {"event": "ready", "pyatv": "<version>"}.
Built on pyatv (MIT).
"""

import asyncio
import json
import sys

import pyatv
from pyatv.const import Protocol

try:
    from importlib.metadata import version as _pkg_version

    PYATV_VERSION = _pkg_version("pyatv")
except Exception:  # pragma: no cover
    PYATV_VERSION = "unknown"


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


async def find(loop, identifier, timeout):
    confs = await pyatv.scan(loop, identifier=identifier, timeout=timeout)
    if not confs:
        raise LookupError(f"device {identifier} not found")
    return confs[0]


async def with_device(loop, cmd, action):
    conf = await find(loop, cmd["identifier"], cmd.get("timeout", 5))
    for proto, creds in (cmd.get("credentials") or {}).items():
        conf.set_credentials(Protocol[proto], creds)
    atv = await pyatv.connect(conf, loop)
    try:
        await action(atv)
    finally:
        tasks = atv.close()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)


async def handle(loop, cmd):
    kind = cmd.get("cmd")
    if kind == "ping":
        return {"event": "pong"}
    if kind == "scan":
        confs = await pyatv.scan(loop, timeout=cmd.get("timeout", 5))
        return {"event": "devices", "devices": [describe(c) for c in confs]}
    if kind == "play_url":
        await with_device(loop, cmd, lambda atv: atv.stream.play_url(cmd["url"]))
        return {"event": "ok"}
    if kind == "stream_file":
        await with_device(loop, cmd, lambda atv: atv.stream.stream_file(cmd["path"]))
        return {"event": "ok"}
    raise ValueError(f"unknown command: {kind}")


async def main():
    loop = asyncio.get_running_loop()
    emit({"event": "ready", "pyatv": PYATV_VERSION})
    while True:
        line = await loop.run_in_executor(None, sys.stdin.readline)
        if not line:
            break
        if not line.strip():
            continue
        try:
            cmd = json.loads(line)
        except json.JSONDecodeError as e:
            emit({"event": "error", "message": f"bad json: {e}"})
            continue
        try:
            reply = await handle(loop, cmd)
        except Exception as e:  # report every failure to the shell; never crash the sidecar
            reply = {"event": "error", "message": str(e) or type(e).__name__}
        emit({"id": cmd.get("id"), **reply})


if __name__ == "__main__":
    asyncio.run(main())
