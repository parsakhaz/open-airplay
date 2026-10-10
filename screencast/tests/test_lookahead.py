"""Lookahead seam: the playlist the receiver sees lists segments that are still being encoded."""
import unittest

from screencast.app import advertise_ahead

LIVE = (b"#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:40\n"
        b"#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"init.mp4\"\n"
        b"#EXTINF:0.500000,\nseg_00040.m4s\n#EXTINF:0.500000,\nseg_00041.m4s\n")


class Lookahead(unittest.TestCase):
    def test_appends_the_next_segments_with_the_nominal_duration(self):
        out = advertise_ahead(LIVE, 2, 0.5).decode()
        self.assertTrue(out.endswith("seg_00041.m4s\n#EXTINF:0.500000,\nseg_00042.m4s\n#EXTINF:0.500000,\nseg_00043.m4s\n"))
        self.assertIn("#EXT-X-MEDIA-SEQUENCE:40", out)  # existing numbering untouched

    def test_zero_lookahead_or_empty_playlist_is_unchanged(self):
        self.assertEqual(advertise_ahead(LIVE, 0, 0.5), LIVE)
        empty = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n"
        self.assertEqual(advertise_ahead(empty, 2, 0.5), empty)

    def test_sub_second_segments_get_a_legal_target_duration(self):
        # ffmpeg writes TARGETDURATION:0 for 0.5 s segments; Apple TV then rejects the playlist outright.
        bad = LIVE.replace(b"#EXT-X-TARGETDURATION:1", b"#EXT-X-TARGETDURATION:0")
        self.assertIn(b"#EXT-X-TARGETDURATION:1\n", advertise_ahead(bad, 0, 0.5))

    def test_works_for_mpegts_segments(self):
        out = advertise_ahead(b"#EXTM3U\n#EXTINF:1.000000,\nseg_00009.ts\n", 1, 1.0).decode()
        self.assertTrue(out.endswith("seg_00010.ts\n"))


if __name__ == "__main__":
    unittest.main()
