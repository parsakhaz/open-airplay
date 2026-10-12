"""A stand-in for the cast helper, for driving the panel without real devices (OAP_SENDER=<this file>).

Speaks the same JSON-lines protocol as oap_sender.py with three made-up devices:
  Living Room   an Apple TV: a cast plays after 1 s and keeps playing until Stop
  Kitchen       a speaker: same, as an audio stream
  Bedroom       an Apple TV that never answers: a cast fails after 3 s ("didn't answer")
Every scan returns the list in a new order (so tests can check rows are keyed by device, not position).
Set OAP_FAKE_SCAN_DELAY (seconds, default 1) to slow scans down.
"""
import json
import os
import random
import sys
import threading
import time

DEVICES = [
    {"identifier": "FAKE-TV-1", "name": "Living Room", "address": "10.9.0.11", "model": "Apple TV 4K", "services": ["AirPlay", "Companion", "RAOP"]},
    {"identifier": "FAKE-SPK-2", "name": "Kitchen", "address": "10.9.0.12", "model": "HomePod mini", "services": ["AirPlay", "RAOP"]},
    {"identifier": "FAKE-TV-3", "name": "Bedroom", "address": "10.9.0.13", "model": "Apple TV HD", "services": ["AirPlay", "RAOP"]},
]
lock = threading.Lock()
jobs = {}  # identifier -> request id


def emit(obj):
    with lock:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


def cast(cmd):
    rid, ident, name = cmd.get("id"), cmd["identifier"], cmd.get("name") or "The device"
    old = jobs.get(ident)
    if old is not None:
        emit({"id": old, "event": "stopped", "identifier": ident})
    jobs[ident] = rid
    if ident == "FAKE-TV-3":
        time.sleep(3)
        if jobs.get(ident) == rid:
            del jobs[ident]
            emit({"id": rid, "event": "error", "identifier": ident, "code": "unreachable",
                  "message": f"{name} didn't answer. Check it's on and on the same network."})
        return
    time.sleep(1)
    if jobs.get(ident) == rid:
        emit({"id": rid, "event": "playing", "identifier": ident})


def handle(cmd):
    kind, rid = cmd.get("cmd"), cmd.get("id")
    sys.stderr.write(f"fake: {rid} {kind} {cmd.get('identifier') or ''}\n")  # lands in sender.log
    sys.stderr.flush()
    if kind == "scan":
        time.sleep(float(os.environ.get("OAP_FAKE_SCAN_DELAY", "1")))
        devices = DEVICES[:]
        random.shuffle(devices)
        emit({"id": rid, "event": "devices", "devices": devices})
    elif kind in ("play_url", "stream_file"):
        cast(cmd)
    elif kind == "stop":
        ident = cmd.get("identifier")
        stopped = [i for i in list(jobs) if ident in (None, i)]
        for i in stopped:
            emit({"id": jobs.pop(i), "event": "stopped", "identifier": i})
        emit({"id": rid, "event": "ok", "stopped": stopped})
    elif kind in ("pause", "resume"):
        if cmd.get("identifier") in jobs:
            emit({"id": rid, "event": "ok"})
        else:
            emit({"id": rid, "event": "error", "code": "not_casting", "message": "Nothing is playing."})
    elif kind == "health":
        emit({"id": rid, "event": "health", "pyatv": "fake", "frozen": False, "casting": sorted(jobs)})
    elif kind == "ping":
        emit({"id": rid, "event": "pong"})
    else:
        emit({"id": rid, "event": "error", "code": "bad_request", "message": f"unknown command: {kind}"})


emit({"event": "ready", "pyatv": "fake"})
for line in sys.stdin:
    if line.strip():
        threading.Thread(target=handle, args=(json.loads(line),), daemon=True).start()
