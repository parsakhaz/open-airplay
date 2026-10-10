"""Receiver playback-state events, in the shapes an Apple TV 4K on tvOS 26.6 actually sent."""
import plistlib
import unittest

from screencast import tvos26


def wrapped(inner: dict) -> bytes:
    return plistlib.dumps({"params": {"data": plistlib.dumps(inner, fmt=plistlib.FMT_BINARY)}}, fmt=plistlib.FMT_BINARY)


class PlaybackEvents(unittest.TestCase):
    def setUp(self):
        tvos26.state.update(playback=None, events=0, last_event=None)

    def test_playback_state_event_sets_state(self):
        tvos26._on_event(wrapped({"type": "playbackState", "name": "loading"}))
        self.assertEqual(tvos26.state["playback"], "loading")
        tvos26._on_event(wrapped({"type": "playbackState", "name": "playing"}))
        self.assertEqual(tvos26.state["playback"], "playing")

    def test_full_status_update_carries_state_in_params(self):
        tvos26._on_event(wrapped({"type": "updatePlaybackInfo", "params": {"playbackState": "playing", "rate": 1.0}}))
        self.assertEqual(tvos26.state["playback"], "playing")

    def test_notifications_and_receiver_info_do_not_change_state(self):
        tvos26._on_event(wrapped({"type": "notification", "name": "readyToPlay", "item": {"uuid": "X"}}))
        tvos26._on_event(plistlib.dumps({"type": "updateInfo", "value": {"name": "SF Board Room"}}, fmt=plistlib.FMT_BINARY))
        self.assertIsNone(tvos26.state["playback"])
        self.assertEqual(tvos26.state["events"], 1)  # only the wrapped playback event counts


if __name__ == "__main__":
    unittest.main()
