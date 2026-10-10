"""Negotiation seam: (this machine, this receiver) -> the options we offer and the defaults we pick."""
import unittest

from screencast.caps import AudioSource, Display, Encoder, MachineCaps, parse_receiver
from screencast.negotiate import negotiate

# Captured from a real Apple TV 4K (gen 3), tvOS 26.6, GET /info on port 7000 (trimmed).
APPLE_TV_4K_INFO = {
    "model": "AppleTV14,1",
    "name": "SF Conference Room 2",
    "features": 4330070159449382869,
    "receiverHDRCapability": "4k60",
}
# Captured from a Sonos Five: AirPlay audio only.
SONOS_INFO = {"model": "Five", "name": "Front", "features": 576956908072569344, "manufacturer": "Sonos"}

UHD = Display(index=1, device=r"\.\DISPLAY2", name="Dell U2720Q", x=2880, y=0,
              width=3840, height=2160, refresh=60, scale=1.5, primary=False)
LAPTOP = Display(index=0, device=r"\.\DISPLAY1", name="Built-in", x=0, y=0,
                 width=2880, height=1800, refresh=120, scale=2.0, primary=True)

AMF_H264 = Encoder("h264_amf", "h264", hardware=True, max_height=2160, max_fps=60)
AMF_HEVC = Encoder("hevc_amf", "hevc", hardware=True, max_height=2160, max_fps=60)
X264 = Encoder("libx264", "h264", hardware=False, max_height=1080, max_fps=60)
X265_SLOW = Encoder("libx265", "hevc", hardware=False, max_height=720, max_fps=30)


def machine(*encoders, audio=()):
    return MachineCaps(displays=(LAPTOP, UHD), encoders=encoders, audio_sources=audio, cpu_count=8)


class ReceiverParsing(unittest.TestCase):
    def test_apple_tv_4k_supports_video_hevc_and_4k60(self):
        rx = parse_receiver(APPLE_TV_4K_INFO)
        self.assertTrue(rx.video)
        self.assertTrue(rx.hevc)
        self.assertEqual((rx.max_width, rx.max_height, rx.max_fps), (3840, 2160, 60))

    def test_speaker_is_not_a_video_receiver(self):
        self.assertFalse(parse_receiver(SONOS_INFO).video)

    def test_receiver_advertised_codecs_and_display_mode_win_over_model_table(self):
        # Fire TV with the AirPlay receiver (trimmed /info): feature bit 0 is clear, yet it plays video,
        # advertises HEVC in its codec strings, and its TV is currently in 1080p mode.
        rx = parse_receiver({
            "model": "AFTOLCBC82", "name": "My Fire TV 3175", "features": 255525703439583952,
            "playbackCapabilities": {"supportsInterstitials": True},
            "displays": [{"widthPixels": 1920, "heightPixels": 1080, "widthPixelsMax": 3840, "heightPixelsMax": 2160,
                          "maxFPS": 60, "HDRSupportedModes": [{"HDRMode": "SDR",
                          "codecStrings": ["avc1.640034", "hvc1.1.4.H153.B0"]}]}],
        })
        self.assertTrue(rx.video)
        self.assertTrue(rx.hevc)
        self.assertEqual((rx.max_width, rx.max_height, rx.max_fps), (1920, 1080, 60))

    def test_unknown_model_falls_back_to_h264_1080p(self):
        rx = parse_receiver({"model": "SomeTV1,1", "features": 1})
        self.assertTrue(rx.video)
        self.assertFalse(rx.hevc)
        self.assertEqual(rx.max_height, 1080)


class Negotiation(unittest.TestCase):
    def test_hevc_hardware_is_the_default_when_both_sides_support_it(self):
        plan = negotiate(machine(AMF_H264, AMF_HEVC), parse_receiver(APPLE_TV_4K_INFO), UHD)
        self.assertEqual(plan.defaults.codec, "hevc")
        self.assertEqual(plan.defaults.encoder, "hevc_amf")
        self.assertEqual((plan.defaults.width, plan.defaults.height, plan.defaults.fps), (3840, 2160, 60))

    def test_receiver_without_hevc_degrades_to_h264_and_says_so(self):
        plan = negotiate(machine(AMF_H264, AMF_HEVC), parse_receiver({"model": "AppleTV5,3", "features": 1}), UHD)
        self.assertEqual(plan.defaults.codec, "h264")
        self.assertNotIn("hevc", [o["value"] for o in plan.options["codec"]])
        self.assertTrue(any("HEVC" in n for n in plan.notes))

    def test_no_hardware_encoder_falls_back_to_software_and_says_so(self):
        plan = negotiate(machine(X264), parse_receiver(APPLE_TV_4K_INFO), UHD)
        self.assertEqual(plan.defaults.encoder, "libx264")
        self.assertTrue(any("software" in n.lower() for n in plan.notes))
        # Software x264 only keeps up at 1080p on this machine: never offer more.
        self.assertLessEqual(plan.defaults.height, 1080)

    def test_slow_software_hevc_is_not_preferred_over_fast_h264(self):
        plan = negotiate(machine(X264, X265_SLOW), parse_receiver(APPLE_TV_4K_INFO), UHD)
        self.assertEqual(plan.defaults.codec, "h264")

    def test_fps_is_capped_by_the_receiver_even_on_a_120hz_display(self):
        plan = negotiate(machine(AMF_HEVC), parse_receiver(APPLE_TV_4K_INFO), LAPTOP)
        self.assertEqual(plan.defaults.fps, 60)
        self.assertTrue(all(o["value"] <= 60 for o in plan.options["fps"]))

    def test_output_keeps_the_display_aspect_ratio_within_receiver_limits(self):
        plan = negotiate(machine(AMF_HEVC), parse_receiver(APPLE_TV_4K_INFO), LAPTOP)
        d = plan.defaults
        # 2880x1800 (16:10) does not fit 3840x2160 height-wise? It does: kept native.
        self.assertEqual((d.width, d.height), (2880, 1800))
        plan = negotiate(machine(X264), parse_receiver(APPLE_TV_4K_INFO), LAPTOP)
        self.assertEqual((plan.defaults.width, plan.defaults.height), (1728, 1080))

    def test_audio_is_only_offered_when_a_loopback_source_exists(self):
        rx = parse_receiver(APPLE_TV_4K_INFO)
        self.assertNotIn("audio", negotiate(machine(AMF_HEVC), rx, UHD).options)
        plan = negotiate(machine(AMF_HEVC, audio=(AudioSource("Stereo Mix (Realtek Audio)"),)), rx, UHD)
        self.assertIn("audio", plan.options)
        self.assertTrue(plan.defaults.audio)

    def test_default_bitrate_targets_near_lossless_desktop_text(self):
        plan = negotiate(machine(AMF_HEVC), parse_receiver(APPLE_TV_4K_INFO), UHD)
        self.assertGreaterEqual(plan.defaults.bitrate_kbps, 30000)  # 4K60 HEVC desktop: >=30 Mb/s

    def test_non_video_receiver_has_nothing_to_offer(self):
        with self.assertRaises(ValueError):
            negotiate(machine(AMF_HEVC), parse_receiver(SONOS_INFO), UHD)


if __name__ == "__main__":
    unittest.main()
