"""Tests for pairing a device that asks (pair_begin, pair_code, pair_cancel) and casting with the saved
credentials, against a fake pyatv pairing handler (no network)."""
import asyncio
import logging
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from oap_sender import Sidecar  # noqa: E402
from pyatv import exceptions  # noqa: E402
from pyatv.const import Protocol  # noqa: E402

from test_jobs import FakeAtv, finishes  # noqa: E402

PIN = "1234"
SECRET = "secret-credentials-abc"


class FakeHandler:
    """pyatv's PairingHandler: begin() makes the device show PIN, finish() checks the pin given."""

    def __init__(self, protocol):
        self.service = SimpleNamespace(protocol=protocol, credentials=None)
        self.begun = 0
        self.closed = False
        self.has_paired = False
        self.given = None

    async def begin(self):
        self.begun += 1

    def pin(self, code):
        self.given = str(code)

    async def finish(self):
        if self.given != PIN:
            raise exceptions.PairingError("bad pin")
        self.service.credentials = SECRET
        self.has_paired = True

    async def close(self):
        self.closed = True


def device(identifier="tv", password=False):
    services = {p: SimpleNamespace(protocol=p, requires_password=password, password=None, credentials=None)
                for p in (Protocol.AirPlay, Protocol.RAOP)}
    conf = SimpleNamespace(identifier=identifier, name="Den TV", address="10.0.0.9", services=list(services.values()),
                           device_info=SimpleNamespace(model_str="Apple TV 4K"), get_service=services.get,
                           credentials={})
    conf.set_credentials = lambda proto, creds: conf.credentials.__setitem__(proto, creds)
    return conf


class Harness:
    def __init__(self, devices):
        self.events, self.handlers, self.connected = [], [], []
        self.devices = list(devices)

        async def scan(timeout=5, identifier=None, hosts=None):
            return [d for d in self.devices if identifier in (None, d.identifier)]

        async def connect(conf):
            self.connected.append((dict(conf.credentials), [s.password for s in conf.services]))
            return FakeAtv(finishes)

        async def pair(conf, protocol):
            handler = FakeHandler(protocol)
            self.handlers.append(handler)
            return handler

        self.sidecar = Sidecar(emit=self.events.append, scan=scan, connect=connect, local=set, pair=pair)

    async def ask(self, cmd):
        await self.sidecar.dispatch(cmd)
        return [e for e in self.events if e.get("id") == cmd["id"]][-1]


def run(coro):
    return asyncio.run(coro)


class Pairing(unittest.TestCase):
    def test_a_pin_pairs_and_returns_the_credentials(self):
        async def go():
            h = Harness([device()])
            begun = await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv", "name": "Den TV", "protocol": "AirPlay"})
            self.assertEqual(begun, {"id": 1, "event": "pairing", "identifier": "tv", "credential": "pin"})
            self.assertEqual((h.handlers[0].service.protocol, h.handlers[0].begun), (Protocol.AirPlay, 1))
            paired = await h.ask({"id": 2, "cmd": "pair_code", "identifier": "tv", "code": PIN})
            self.assertEqual(paired, {"id": 2, "event": "paired", "identifier": "tv", "credentials": {"AirPlay": SECRET}})
            self.assertTrue(h.handlers[0].closed)
            self.assertEqual(h.sidecar.pairings, {})
        run(go())

    def test_a_speaker_pairs_its_audio_stream(self):
        async def go():
            h = Harness([device()])
            await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv", "protocol": "RAOP"})
            paired = await h.ask({"id": 2, "cmd": "pair_code", "identifier": "tv", "code": PIN})
            self.assertEqual(paired["credentials"], {"RAOP": SECRET})
        run(go())

    def test_a_wrong_code_starts_again_so_the_device_shows_a_new_one(self):
        async def go():
            h = Harness([device()])
            await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv", "name": "Den TV"})
            wrong = await h.ask({"id": 2, "cmd": "pair_code", "identifier": "tv", "code": "9999"})
            self.assertEqual((wrong["event"], wrong["code"]), ("error", "wrong_code"))
            self.assertEqual(wrong["message"], "That code didn't work. Enter the new code shown on Den TV.")
            self.assertTrue(h.handlers[0].closed)
            self.assertEqual(h.handlers[1].begun, 1)  # a new attempt is open
            paired = await h.ask({"id": 3, "cmd": "pair_code", "identifier": "tv", "code": PIN})
            self.assertEqual(paired["credentials"], {"AirPlay": SECRET})
        run(go())

    def test_cancel_while_the_device_is_still_starting_closes_the_late_pairing(self):
        async def go():
            h = Harness([device()])
            release = asyncio.Event()

            async def slow_begin(handler=None):
                await release.wait()  # the device takes its time to show the code

            original_pair = h.sidecar.pair_fn

            async def pair(conf, protocol):
                handler = await original_pair(conf, protocol)
                handler.begin = slow_begin
                return handler

            h.sidecar.pair_fn = pair
            begin = asyncio.create_task(h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv", "name": "Den TV"}))
            while not h.handlers:  # begin() is waiting on the device
                await asyncio.sleep(0)
            cancelled = await h.ask({"id": 2, "cmd": "pair_cancel", "identifier": "tv"})
            self.assertEqual(cancelled["event"], "ok")
            release.set()  # ...and then the device finishes starting
            late = await begin
            self.assertEqual((late["event"], late.get("code")), ("error", "cancelled"))
            self.assertTrue(h.handlers[0].closed, "the pairing that finished after Cancel is closed")
            self.assertEqual(h.sidecar.pairings, {}, "no pairing is left open")
        run(go())

    def test_cancel_closes_the_pairing(self):
        async def go():
            h = Harness([device()])
            await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv"})
            self.assertEqual(await h.ask({"id": 2, "cmd": "pair_cancel", "identifier": "tv"}), {"id": 2, "event": "ok"})
            self.assertTrue(h.handlers[0].closed)
            late = await h.ask({"id": 3, "cmd": "pair_code", "identifier": "tv", "code": PIN})
            self.assertEqual(late["code"], "not_pairing")
        run(go())

    def test_a_device_with_a_password_asks_for_it_instead(self):
        async def go():
            h = Harness([device(password=True)])
            begun = await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv", "protocol": "RAOP"})
            self.assertEqual(begun["credential"], "password")
            self.assertEqual(h.handlers, [])  # nothing is shown on the device
            paired = await h.ask({"id": 2, "cmd": "pair_code", "identifier": "tv", "code": "hunter2"})
            self.assertEqual(paired["credentials"], {"password": "hunter2"})
        run(go())

    def test_an_unknown_device_is_unreachable(self):
        async def go():
            h = Harness([])
            reply = await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "gone", "name": "Den TV"})
            self.assertEqual(reply["code"], "unreachable")
        run(go())

    def test_credentials_and_codes_never_reach_the_log(self):
        async def go():
            h = Harness([device()])
            with self.assertLogs("oap_sender", level=logging.INFO) as logs:
                await h.ask({"id": 1, "cmd": "pair_begin", "identifier": "tv"})
                await h.ask({"id": 2, "cmd": "pair_code", "identifier": "tv", "code": "9876"})
                await h.ask({"id": 3, "cmd": "pair_code", "identifier": "tv", "code": PIN})
            text = "\n".join(logs.output)
            for secret in (SECRET, PIN, "9876"):
                self.assertNotIn(secret, text)
        run(go())


class CastWithCredentials(unittest.TestCase):
    def test_saved_credentials_are_given_to_the_device(self):
        """The regression: a cast after pairing carries the saved credentials, so it no longer needs pairing."""
        async def go():
            h = Harness([device()])
            await h.ask({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x/a.mp4",
                         "credentials": {"AirPlay": SECRET, "Unknown": "ignored"}})
            self.assertEqual(h.connected[0][0], {Protocol.AirPlay: SECRET})
        run(go())

    def test_a_saved_password_is_given_to_every_service(self):
        async def go():
            h = Harness([device(password=True)])
            await h.ask({"id": 1, "cmd": "stream_file", "identifier": "tv", "path": "C:/a.mp3",
                         "credentials": {"password": "hunter2"}})
            self.assertEqual(h.connected[0][1], ["hunter2", "hunter2"])
        run(go())


if __name__ == "__main__":
    unittest.main()
