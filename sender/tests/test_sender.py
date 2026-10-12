"""Tests for the sender sidecar's JSON-lines protocol (the seam the Rust shell talks to)."""
import json
import os
import subprocess
import sys
import threading
import unittest
from pathlib import Path

SENDER = Path(__file__).resolve().parents[1] / "oap_sender.py"
# OAP_SENDER_EXE points these tests at the frozen build (sender/dist/oap-sender/oap-sender.exe).
FROZEN = os.environ.get("OAP_SENDER_EXE")


def run_sidecar(*commands, wait_for=(), timeout=30):
    """Sends the commands, keeps stdin open until every id in wait_for has a final reply, then closes it."""
    argv = [FROZEN] if FROZEN else [sys.executable, str(SENDER)]
    proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    events, waiting = [], set(wait_for)
    timer = threading.Timer(timeout, proc.kill)
    timer.start()
    try:
        proc.stdin.write("".join(json.dumps(c) + "\n" for c in commands))
        proc.stdin.flush()
        if not waiting:
            proc.stdin.close()
        for line in proc.stdout:
            if not line.strip():
                continue
            event = json.loads(line)
            events.append(event)
            if event.get("id") in waiting and event.get("event") != "playing":
                waiting.discard(event["id"])
                if not waiting:
                    proc.stdin.close()
        proc.wait()
        proc.stdout.close()
    finally:
        timer.cancel()
    return events


class SidecarProtocol(unittest.TestCase):
    def test_announces_ready_with_pyatv_version(self):
        events = run_sidecar()
        self.assertEqual(events[0]["event"], "ready")
        self.assertRegex(events[0]["pyatv"], r"^\d+\.\d+")

    def test_scan_returns_a_device_list(self):
        events = run_sidecar({"id": 1, "cmd": "scan", "timeout": 1}, wait_for=[1])
        reply = [e for e in events if e.get("id") == 1][0]
        self.assertEqual(reply["event"], "devices")
        self.assertIsInstance(reply["devices"], list)

    def test_unknown_command_is_an_error_not_a_crash(self):
        events = run_sidecar({"id": 7, "cmd": "nope"}, {"id": 8, "cmd": "ping"}, wait_for=[7, 8])
        self.assertEqual([e for e in events if e.get("id") == 7][0]["event"], "error")
        self.assertEqual([e for e in events if e.get("id") == 8][0]["event"], "pong")

    def test_valid_json_that_is_not_an_object_is_an_error_not_a_crash(self):
        events = run_sidecar([], None, 7, {"id": 9, "cmd": "ping"}, wait_for=[9])
        errors = [e for e in events if e.get("event") == "error"]
        self.assertEqual(len(errors), 3)
        self.assertTrue(all("object" in e["message"] for e in errors))
        self.assertEqual([e for e in events if e.get("id") == 9][0]["event"], "pong")

    def test_play_on_unknown_device_is_a_plain_error(self):
        events = run_sidecar(
            {"id": 2, "cmd": "play_url", "identifier": "no-such-device", "name": "Den TV", "url": "http://x/y.mp4"},
            wait_for=[2],
        )
        reply = [e for e in events if e.get("id") == 2][0]
        self.assertEqual((reply["event"], reply["code"]), ("error", "unreachable"))
        self.assertEqual(reply["message"], "Den TV didn't answer. Check it's on and on the same network.")

    def test_health_reports_version_and_nothing_casting(self):
        events = run_sidecar({"id": 3, "cmd": "health"}, wait_for=[3])
        reply = [e for e in events if e.get("id") == 3][0]
        self.assertEqual(reply["casting"], [])
        self.assertEqual(reply["frozen"], bool(FROZEN))


if __name__ == "__main__":
    unittest.main()
