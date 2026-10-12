"""Tests for the sidecar's per-device jobs, Stop, timeouts and plain errors, with fake devices (no network)."""
import asyncio
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import oap_sender  # noqa: E402
from oap_sender import Sidecar, plain_error  # noqa: E402
from pyatv import exceptions  # noqa: E402


class FakeStream:
    def __init__(self, behaviour):
        self.behaviour = behaviour
        self.played = []

    async def play_url(self, url):
        self.played.append(url)
        await self.behaviour()

    stream_file = play_url


class FakeAtv:
    def __init__(self, behaviour):
        self.stream = FakeStream(behaviour)
        self.closed = False
        self.remote_control = SimpleNamespace(pause=self._ok, play=self._ok)

    async def _ok(self):
        return None

    def close(self):
        self.closed = True
        return set()


def conf(identifier, address, name="Den TV"):
    return SimpleNamespace(identifier=identifier, name=name, address=address, services=[],
                           device_info=SimpleNamespace(model_str="Apple TV 4K"), set_credentials=lambda *a: None)


async def forever():
    await asyncio.Event().wait()


async def finishes():
    await asyncio.sleep(0)


class Harness:
    """A Sidecar with fake scan/connect that records every event it emits."""

    def __init__(self, devices=(), behaviour=forever, connect_delay=0, scan_delay=0, local=("10.0.0.5",)):
        self.events = []
        self.atvs = []
        self.devices = list(devices)

        self.scans = []

        async def scan(timeout=5, identifier=None, hosts=None):
            self.scans.append({"timeout": timeout, "identifier": identifier, "hosts": hosts})
            await asyncio.sleep(scan_delay)
            return [d for d in self.devices if identifier in (None, d.identifier) and (hosts is None or d.address in hosts)]

        async def connect(_conf):
            await asyncio.sleep(connect_delay)
            atv = FakeAtv(behaviour)
            self.atvs.append(atv)
            return atv

        self.sidecar = Sidecar(emit=self.events.append, scan=scan, connect=connect, local=lambda: set(local))

    def replies(self, rid):
        return [e for e in self.events if e.get("id") == rid]

    async def until(self, predicate, seconds=3):
        for _ in range(int(seconds / 0.01)):
            if predicate():
                return
            await asyncio.sleep(0.01)
        raise AssertionError(f"timed out; events: {self.events}")


def run(coro):
    return asyncio.run(coro)


class Jobs(unittest.TestCase):
    def setUp(self):
        self._grace = oap_sender.START_GRACE_SECONDS
        oap_sender.START_GRACE_SECONDS = 0.05

    def tearDown(self):
        oap_sender.START_GRACE_SECONDS = self._grace

    def test_a_long_cast_does_not_block_a_scan(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x/a.mp4"})
            await h.until(lambda: any(e["event"] == "playing" for e in h.replies(1)))
            h.sidecar.dispatch({"id": 2, "cmd": "scan", "timeout": 1})
            await h.until(lambda: h.replies(2))
            self.assertEqual(h.replies(2)[0]["event"], "devices")
            self.assertEqual([e["event"] for e in h.replies(1)], ["playing"])  # still casting
            await h.sidecar.shutdown()
        run(go())

    def test_stop_ends_the_cast_and_closes_the_connection(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x/a.mp4"})
            await h.until(lambda: h.replies(1))
            h.sidecar.dispatch({"id": 2, "cmd": "stop", "identifier": "tv"})
            await h.until(lambda: h.replies(2))
            self.assertEqual([e["event"] for e in h.replies(1)], ["playing", "stopped"])
            self.assertEqual(h.replies(2)[0], {"id": 2, "event": "ok", "stopped": ["tv"]})
            self.assertTrue(h.atvs[0].closed)
            self.assertEqual(h.sidecar.jobs, {})
        run(go())

    def test_stop_without_a_device_stops_every_cast(self):
        async def go():
            h = Harness([conf("a", "10.0.0.8"), conf("b", "10.0.0.9")])
            for rid, ident in ((1, "a"), (2, "b")):
                h.sidecar.dispatch({"id": rid, "cmd": "play_url", "identifier": ident, "url": "http://x/a.mp4"})
            await h.until(lambda: h.replies(1) and h.replies(2))
            h.sidecar.dispatch({"id": 3, "cmd": "stop"})
            await h.until(lambda: h.replies(3))
            self.assertEqual(sorted(h.replies(3)[0]["stopped"]), ["a", "b"])
            self.assertEqual(h.replies(1)[-1]["event"], "stopped")
            self.assertEqual(h.replies(2)[-1]["event"], "stopped")
        run(go())

    def test_casting_to_one_device_leaves_another_alone(self):
        async def go():
            h = Harness([conf("a", "10.0.0.8"), conf("b", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "a", "url": "http://x/a.mp4"})
            h.sidecar.dispatch({"id": 2, "cmd": "play_url", "identifier": "b", "url": "http://x/b.mp4"})
            await h.until(lambda: h.replies(1) and h.replies(2))
            h.sidecar.dispatch({"id": 3, "cmd": "stop", "identifier": "a"})
            await h.until(lambda: h.replies(3))
            self.assertEqual(h.replies(2)[-1]["event"], "playing")
            self.assertEqual(sorted(h.sidecar.jobs), ["b"])
            await h.sidecar.shutdown()
        run(go())

    def test_a_new_cast_on_the_same_device_replaces_the_old_one(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x/a.mp4"})
            await h.until(lambda: h.replies(1))
            h.sidecar.dispatch({"id": 2, "cmd": "play_url", "identifier": "tv", "url": "http://x/b.mp4"})
            await h.until(lambda: h.replies(2))
            self.assertEqual(h.replies(1)[-1]["event"], "stopped")
            self.assertEqual(h.sidecar.jobs["tv"].request_id, 2)
            await h.sidecar.shutdown()
        run(go())

    def test_a_cast_that_ends_by_itself_reports_ok(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")], behaviour=finishes)
            h.sidecar.dispatch({"id": 1, "cmd": "stream_file", "identifier": "tv", "path": "C:/song.mp3"})
            await h.until(lambda: h.replies(1))
            self.assertEqual(h.replies(1), [{"id": 1, "event": "ok", "identifier": "tv"}])
            self.assertEqual(h.sidecar.jobs, {})
        run(go())

    def test_an_unreachable_device_ends_with_a_plain_sentence_in_time(self):
        async def go():
            oap_sender.CONNECT_SECONDS, saved = 0.1, oap_sender.CONNECT_SECONDS
            try:
                h = Harness([conf("tv", "10.0.0.9")], connect_delay=60)
                h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "tv", "name": "Den TV", "url": "http://x/a.mp4"})
                await h.until(lambda: h.replies(1))
            finally:
                oap_sender.CONNECT_SECONDS = saved
            reply = h.replies(1)[0]
            self.assertEqual((reply["event"], reply["code"]), ("error", "unreachable"))
            self.assertEqual(reply["message"], "Den TV didn't answer. Check it's on and on the same network.")
        run(go())

    def test_an_unknown_device_is_unreachable(self):
        async def go():
            h = Harness([])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "gone", "name": "Den TV", "url": "http://x"})
            await h.until(lambda: h.replies(1))
            self.assertEqual(h.replies(1)[0]["code"], "unreachable")
        run(go())

    def test_a_scan_never_runs_past_its_limit(self):
        async def go():
            oap_sender.MAX_SCAN_SECONDS, saved = 1, oap_sender.MAX_SCAN_SECONDS
            try:
                h = Harness([], scan_delay=60)
                started = asyncio.get_running_loop().time()
                h.sidecar.dispatch({"id": 1, "cmd": "scan", "timeout": 30})
                await h.until(lambda: h.replies(1), seconds=6)
            finally:
                oap_sender.MAX_SCAN_SECONDS = saved
            self.assertLess(asyncio.get_running_loop().time() - started, 4)
            self.assertEqual(h.replies(1)[0]["code"], "scan_failed")
        run(go())

    def test_a_scan_leaves_out_this_pc(self):
        async def go():
            h = Harness([conf("me", "10.0.0.5", "This PC"), conf("tv", "10.0.0.9")], local=("10.0.0.5",))
            h.sidecar.dispatch({"id": 1, "cmd": "scan", "timeout": 1})
            await h.until(lambda: h.replies(1))
            self.assertEqual([d["identifier"] for d in h.replies(1)[0]["devices"]], ["tv"])
        run(go())

    def test_a_scan_can_ask_addresses_directly(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9"), conf("me", "10.0.0.5"), conf("other", "10.0.0.7")], local=("10.0.0.5",))
            h.sidecar.dispatch({"id": 1, "cmd": "scan", "timeout": 30, "hosts": ["10.0.0.9", "10.0.0.5"]})
            await h.until(lambda: h.replies(1))
            self.assertEqual(h.scans[-1], {"timeout": oap_sender.MAX_SCAN_SECONDS, "identifier": None, "hosts": ["10.0.0.9", "10.0.0.5"]})
            self.assertEqual([d["identifier"] for d in h.replies(1)[0]["devices"]], ["tv"])  # this PC still left out
            h.sidecar.dispatch({"id": 2, "cmd": "scan", "timeout": 1, "hosts": ["10.0.0.99"]})
            await h.until(lambda: h.replies(2))
            self.assertEqual(h.replies(2)[0], {"id": 2, "event": "devices", "devices": []})
        run(go())

    def test_a_cast_reaches_an_unscanned_device_by_its_address(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "tv", "address": "10.0.0.9", "url": "http://x/a.mp4"})
            await h.until(lambda: h.replies(1))
            self.assertEqual(h.replies(1)[0]["event"], "playing")
            self.assertEqual(h.scans, [{"timeout": 5, "identifier": None, "hosts": ["10.0.0.9"]}])
            await h.sidecar.shutdown()
        run(go())

    def test_health_lists_the_devices_being_cast_to(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x/a.mp4"})
            await h.until(lambda: h.replies(1))
            h.sidecar.dispatch({"id": 2, "cmd": "health"})
            await h.until(lambda: h.replies(2))
            self.assertEqual(h.replies(2)[0]["casting"], ["tv"])
            await h.sidecar.shutdown()
        run(go())

    def test_pause_needs_a_cast(self):
        async def go():
            h = Harness([conf("tv", "10.0.0.9")])
            h.sidecar.dispatch({"id": 1, "cmd": "pause", "identifier": "tv", "name": "Den TV"})
            await h.until(lambda: h.replies(1))
            self.assertEqual(h.replies(1)[0]["message"], "Nothing is playing on Den TV.")
            h.sidecar.dispatch({"id": 2, "cmd": "play_url", "identifier": "tv", "url": "http://x/a.mp4"})
            await h.until(lambda: h.replies(2))
            h.sidecar.dispatch({"id": 3, "cmd": "pause", "identifier": "tv"})
            await h.until(lambda: h.replies(3))
            self.assertEqual(h.replies(3)[0]["event"], "ok")
            await h.sidecar.shutdown()
        run(go())


class PlainErrors(unittest.TestCase):
    BANNED = ("engine", "exit code", "Some(", "None", "Apply", "sidecar", "pyatv", "iOS 27", "Error", "Exception")

    def test_errors_map_to_plain_sentences(self):
        cases = {
            exceptions.AuthenticationError("x"): "needs_pairing",
            exceptions.NoCredentialsError("x"): "needs_pairing",
            exceptions.NotSupportedError("x"): "unsupported",
            exceptions.ConnectionLostError("x"): "lost",
            exceptions.ConnectionFailedError("x"): "unreachable",
            exceptions.PlaybackError("x"): "playback",
            exceptions.BlockedStateError("x"): "busy",
            asyncio.TimeoutError(): "unreachable",
            ConnectionRefusedError(): "unreachable",
            FileNotFoundError("C:/nope.mp3"): "bad_file",
            RuntimeError("boom"): "failed",
        }
        for exc, code in cases.items():
            got, message = plain_error(exc, "Den TV", "that link")
            self.assertEqual(got, code, exc)
            self.assertTrue(message.endswith("."), message)
            for word in self.BANNED:
                self.assertNotIn(word, message)


if __name__ == "__main__":
    unittest.main()
