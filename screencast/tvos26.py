"""AirPlay URL playback that works on tvOS 26, applied over pyatv 0.18.0 (pinned).

pyatv 0.18's AirPlay 2 play_url sends the legacy `POST /play`. tvOS 26 answers 200 but never creates a player:
`/playback-info` returns 500 forever and the URL is never fetched (pyatv issue #2906; reproduced here on an
Apple TV 4K, tvOS 26.6 / 23L773). Modern senders instead SETUP a remote-control stream (type 130) on the RTSP
session and drive playback with plist commands sent to `POST /command`; playback state comes back as events.

The protocol change follows robkochman/pyatv@8144c77c ("Fix AirPlay URL playback on modern tvOS", MIT, same
license as pyatv). Reimplemented here rather than installing an unreviewed fork; remove once upstream ships a fix.
"""
from __future__ import annotations

import asyncio
import logging
import plistlib
from uuid import uuid4

from pyatv import exceptions
from pyatv.protocols.airplay import channels
from pyatv.protocols.airplay.player import AirPlayPlayer
from pyatv.protocols.raop.protocols import airplayv2
from pyatv.support.http import HttpResponse, decode_bplist_from_body
from pyatv.support.rtsp import HTTP_PROTOCOL

_LOGGER = logging.getLogger(__name__)

COMMAND_USER_AGENT = "AirPlay/870.14.1"
REMOTE_CONTROL_CLIENT_TYPE = "A6B27562-B43A-4F2D-B75F-82391E250194"
START_TIMEOUT = 30  # seconds a receiver may take to start a live stream

state = {"playback": None, "events": 0, "last_event": None}  # read by the UI for diagnostics
_changed = asyncio.Event()


def _on_event(body: bytes):
    try:
        event = plistlib.loads(body)
    except Exception:
        return
    # Playback events arrive as {"params": {"data": <bplist>}}; the inner plist is one of
    #   {"type": "playbackState", "name": "loading" | "playing" | "paused" | ...}
    #   {"type": "notification", "name": "readyToPlay" | "currentItemChanged" | ..., "item": {...}}
    #   {..., "params": {"playbackState": "playing", "rate": 1.0, ...}}   (a full status update)
    # Other events (e.g. {"type": "updateInfo"}) are receiver info and are ignored.
    outer = event.get("params") if isinstance(event, dict) else None
    if not (isinstance(outer, dict) and isinstance(outer.get("data"), bytes)):
        return
    try:
        inner = plistlib.loads(outer["data"])
    except Exception:
        return
    state["events"] += 1
    state["last_event"] = f"{inner.get('type')}:{inner.get('name')}"
    s = None
    if inner.get("type") == "playbackState":
        s = inner.get("name")
    elif isinstance(inner.get("params"), dict):
        s = inner["params"].get("playbackState")
    if isinstance(s, str) and s:
        if s.lower() != state["playback"]:
            _LOGGER.info("receiver playback state: %s", s.lower())
        state["playback"] = s.lower()
        _changed.set()


_original_handle_received = channels.EventChannel.handle_received


def _handle_received(self):
    # Peek at each complete request before pyatv's handler consumes and acknowledges it.
    buf = self.buffer
    while buf:
        request, _, buf = self.parse_request(buf)
        if request is None:
            break
        if request.body:
            _on_event(request.body if isinstance(request.body, bytes) else request.body.encode())
    _original_handle_received(self)


async def _send_command(self, command: dict) -> HttpResponse:
    body = {"params": {"data": plistlib.dumps(command, fmt=plistlib.FMT_BINARY, sort_keys=False)}}
    return await self.rtsp.exchange(
        "POST", uri="/command", protocol=HTTP_PROTOCOL, allow_error=True, body=body,
        headers={"User-Agent": COMMAND_USER_AGENT, "X-Apple-ProtocolVersion": "1",
                 "X-Apple-Session-ID": self._oap_session_id, "X-Apple-StreamID": self._oap_stream_id})


async def _play_url(self, timing_server_port: int, url: str, position: float = 0.0) -> HttpResponse:
    if self._verifier is None:
        await self._setup_base(timing_server_port)
        await self.start_feedback()
    await self.rtsp.info()
    await self.rtsp.record()
    state.update(playback=None, events=0, last_event=None)
    sender = "AA:BB:CC:DD:EE:FF"  # the deviceID pyatv 0.18 announces in its base SETUP
    resp = await self.rtsp.setup(body={"streams": [{
        "clientUUID": str(uuid4()).upper(), "clientTypeUUID": REMOTE_CONTROL_CLIENT_TYPE,
        "channelID": f"{sender}-RCS-1", "controlType": 1, "type": 130}]})
    streams = decode_bplist_from_body(resp).get("streams", [])
    if not streams or "streamID" not in streams[0]:
        raise exceptions.ProtocolError("receiver did not open a URL control stream")
    self._oap_stream_id = int(streams[0]["streamID"])
    self._oap_session_id = str(uuid4()).upper()
    item = {"uuid": str(uuid4()).upper(), "mediaType": "file", "Content-Location": url,
            "Start-Position-Seconds": position}
    for command in (
        {"type": "insertPlayQueueItem", "item": item},
        {"type": "setProperty", "value": True, "property": "isInterestedInDateRange", "item": {"uuid": item["uuid"]}},
        {"type": "setProperty", "value": 1, "property": "actionAtItemEnd"},
        {"type": "setRate", "rate": 1.0},
    ):
        resp = await _send_command(self, command)
        if not 200 <= resp.code < 300:
            return resp
    return resp


async def _wait_for_media_to_end(self) -> None:
    """Hold the session open while the receiver plays; return when it stops, raise if it never starts."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + START_TIMEOUT
    started = False
    while True:
        s = state["playback"]
        if s == "playing":
            started = True
        elif started and s in ("idle", "stopped"):
            return
        elif not started and loop.time() > deadline:
            raise exceptions.PlaybackError(
                f"receiver never started playing (state={s}, events={state['events']}); "
                "it may be unable to reach this PC")
        _changed.clear()
        try:
            await asyncio.wait_for(_changed.wait(), 1.0)
        except asyncio.TimeoutError:
            pass


def install():
    channels.EventChannel.handle_received = _handle_received
    airplayv2.AirPlayV2.play_url = _play_url
    AirPlayPlayer._wait_for_media_to_end = _wait_for_media_to_end
