"""Tests for the sender sidecar's JSON-lines protocol (the seam the Rust shell talks to)."""
import json
import subprocess
import sys
import unittest
from pathlib import Path

SENDER = Path(__file__).resolve().parents[1] / "oap_sender.py"


def run_sidecar(*commands, timeout=30):
    stdin = "".join(json.dumps(c) + "\n" for c in commands)
    out = subprocess.run(
        [sys.executable, str(SENDER)], input=stdin, capture_output=True, text=True, timeout=timeout
    )
    return [json.loads(line) for line in out.stdout.splitlines() if line.strip()]


class SidecarProtocol(unittest.TestCase):
    def test_announces_ready_with_pyatv_version(self):
        events = run_sidecar()
        self.assertEqual(events[0]["event"], "ready")
        self.assertRegex(events[0]["pyatv"], r"^\d+\.\d+")

    def test_scan_returns_a_device_list(self):
        events = run_sidecar({"id": 1, "cmd": "scan", "timeout": 1})
        reply = [e for e in events if e.get("id") == 1][0]
        self.assertEqual(reply["event"], "devices")
        self.assertIsInstance(reply["devices"], list)

    def test_unknown_command_is_an_error_not_a_crash(self):
        events = run_sidecar({"id": 7, "cmd": "nope"}, {"id": 8, "cmd": "ping"})
        self.assertEqual([e for e in events if e.get("id") == 7][0]["event"], "error")
        self.assertEqual([e for e in events if e.get("id") == 8][0]["event"], "pong")

    def test_valid_json_that_is_not_an_object_is_an_error_not_a_crash(self):
        events = run_sidecar([], None, 7, {"id": 9, "cmd": "ping"})
        errors = [e for e in events if e.get("event") == "error"]
        self.assertEqual(len(errors), 3)
        self.assertTrue(all("object" in e["message"] for e in errors))
        self.assertEqual([e for e in events if e.get("id") == 9][0]["event"], "pong")

    def test_play_on_unknown_device_reports_not_found(self):
        events = run_sidecar({"id": 2, "cmd": "play_url", "identifier": "no-such-device", "url": "http://x/y.mp4", "timeout": 1})
        reply = [e for e in events if e.get("id") == 2][0]
        self.assertEqual(reply["event"], "error")
        self.assertIn("not found", reply["message"])


if __name__ == "__main__":
    unittest.main()
