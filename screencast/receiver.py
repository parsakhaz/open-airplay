"""Find AirPlay receivers, read their advertised capabilities, and play/stop a URL on one (via pyatv)."""
from __future__ import annotations

import asyncio
import json
import os
import plistlib
import socket
import tempfile
from pathlib import Path

import aiohttp
import pyatv
from pyatv.auth.hap_pairing import TRANSIENT_CREDENTIALS
from pyatv.const import Protocol

from . import tvos26
from .caps import ReceiverCaps, parse_receiver


async def _info(session: aiohttp.ClientSession, address: str, port: int) -> dict | None:
    """GET /info: tvOS answers unauthenticated with model, features and receiverHDRCapability."""
    try:
        async with session.get(f"http://{address}:{port}/info", timeout=aiohttp.ClientTimeout(total=3)) as r:
            if r.status == 200:
                return plistlib.loads(await r.read())
    except Exception:
        pass
    return None


async def scan(timeout: float = 4) -> list[dict]:
    """Every AirPlay receiver on the LAN with its caps; audio-only ones are kept but marked video=False."""
    loop = asyncio.get_running_loop()
    confs = await pyatv.scan(loop, timeout=timeout)
    found = []
    async with aiohttp.ClientSession() as session:
        async def one(conf):
            svc = conf.get_service(Protocol.AirPlay)
            if not svc:
                return None
            info = await _info(session, str(conf.address), svc.port)
            caps = parse_receiver({**dict(svc.properties), "name": conf.name, **(info or {})})
            return {"identifier": conf.identifier, "name": conf.name, "address": str(conf.address),
                    "caps": caps, "probed": info is not None}

        for r in await asyncio.gather(*(one(c) for c in confs)):
            if r:
                found.append(r)
    found.sort(key=lambda r: (not r["caps"].video, r["name"].lower()))
    return found


def local_address_for(remote: str) -> str:
    """The LAN address of the interface that routes to the receiver (what it must fetch the stream from)."""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.connect((remote, 7000))
        return s.getsockname()[0]


tvos26.install()


CREDENTIALS = Path(os.environ.get("LOCALAPPDATA", tempfile.gettempdir())) / "open-airplay" / "airplay-credentials.json"


def load_credentials() -> dict:
    try:
        return json.loads(CREDENTIALS.read_text())
    except Exception:
        return {}


def save_credentials(identifier: str, credentials: str):
    creds = load_credentials()
    creds[identifier] = credentials
    CREDENTIALS.parent.mkdir(parents=True, exist_ok=True)
    CREDENTIALS.write_text(json.dumps(creds, indent=1))


class PairingRequired(Exception):
    """tvOS answered 470: this PC must be PIN-paired once before it may play video."""


async def _conf(identifier):
    confs = await pyatv.scan(asyncio.get_running_loop(), identifier=identifier, timeout=5)
    if not confs:
        raise LookupError("receiver not found on the network")
    return confs[0]


class Pairing:
    """One-time PIN pairing: begin() makes the receiver show a PIN on screen; finish(pin) stores the credential."""

    def __init__(self, identifier):
        self.identifier = identifier
        self._handler = None

    async def begin(self):
        self._handler = await pyatv.pair(await _conf(self.identifier), Protocol.AirPlay, asyncio.get_running_loop())
        await self._handler.begin()

    async def finish(self, pin: str):
        try:
            self._handler.pin(int(pin))
            await self._handler.finish()
            if not self._handler.has_paired:
                raise ValueError("pairing failed: wrong PIN?")
            save_credentials(self.identifier, self._handler.service.credentials)
        finally:
            await self.close()

    async def close(self):
        if self._handler:
            await self._handler.close()
            self._handler = None


class Cast:
    """One AirPlay video session. pyatv's play_url holds the session open until playback ends."""

    def __init__(self, identifier: str, url: str):
        self.identifier, self.url = identifier, url
        self.task: asyncio.Task | None = None
        self.error: str | None = None
        self._atv = None

    async def start(self, settle: float = 4):
        conf = await _conf(self.identifier)
        # pyatv's play_url otherwise sends no credentials (a "null" verify, which tvOS rejects with 403).
        # Prefer a stored PIN pairing; fall back to transient pairing, which some receivers accept.
        conf.set_credentials(Protocol.AirPlay, load_credentials().get(self.identifier) or str(TRANSIENT_CREDENTIALS))
        self._atv = await pyatv.connect(conf, asyncio.get_running_loop())
        tvos26.state.update(proto=None, playback=None)
        self.task = asyncio.create_task(self._play())
        # play_url only returns when playback ends, so watch it briefly for an immediate refusal.
        done, _ = await asyncio.wait({self.task}, timeout=settle)
        if done and self.error and ("470" in self.error or "not authenticated" in self.error):
            await self.stop()
            raise PairingRequired("this receiver needs a one-time PIN pairing with this PC")

    async def _play(self):
        try:
            await self._atv.stream.play_url(self.url)
            self.error = self.error or "receiver ended playback"
        except asyncio.CancelledError:
            raise
        except Exception as e:  # surfaced in the UI
            self.error = str(e) or type(e).__name__

    async def stop(self):
        if self.task:
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)
        if self._atv:
            tasks = self._atv.close()
            if tasks:
                await asyncio.gather(*tasks, return_exceptions=True)
            self._atv = None


def caps_json(c: ReceiverCaps) -> dict:
    return {"model": c.model, "video": c.video, "hevc": c.hevc, "max": f"{c.max_width}x{c.max_height}@{c.max_fps}",
            "hdr": c.hdr, "source": c.source}
