"""Pipeline seam: the ffmpeg command we build produces an HLS stream an Apple TV can play.

Runs the real ffmpeg with a synthetic source in place of the screen grabber, then inspects the output
with ffprobe the way a player would see it.
"""
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from screencast.negotiate import Settings
from screencast.pipeline import MASTER, build_args

HAVE_FFMPEG = shutil.which("ffmpeg") and shutil.which("ffprobe")


def settings(**kw):
    base = dict(codec="h264", encoder="libx264", width=640, height=360, fps=30, bitrate_kbps=3000,
                latency="balanced", audio=False)
    base.update(kw)
    return Settings(**base)


def run_for(s, seconds=3.5):
    out = Path(tempfile.mkdtemp(prefix="oap-hls-"))
    src = ["-re", "-f", "lavfi", "-t", str(seconds), "-i", "testsrc2=size=1280x720:rate=30,format=bgra"]
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", *build_args(s, video_input=src)],
                   check=True, timeout=60, cwd=out)
    return out


def probe(path):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)],
                       capture_output=True, text=True, check=True)
    return json.loads(r.stdout)["streams"][0]


@unittest.skipUnless(HAVE_FFMPEG, "ffmpeg not installed")
class HlsOutput(unittest.TestCase):
    def test_h264_stream_is_scaled_and_cut_into_one_second_segments(self):
        out = run_for(settings())
        master = (out / MASTER).read_text()
        self.assertIn("avc1", master)  # CODECS attribute lets the receiver decide before downloading
        media = (out / "stream.m3u8").read_text()
        self.assertIn("#EXT-X-TARGETDURATION:1", media)
        seg = sorted(out.glob("seg_*.ts"))[0]
        st = probe(seg)
        self.assertEqual((st["codec_name"], st["width"], st["height"]), ("h264", 640, 360))
        self.assertEqual(st.get("color_space"), "bt709")

    def test_hevc_uses_fmp4_with_hvc1_tag_as_apple_requires(self):
        out = run_for(settings(codec="hevc", encoder="libx265"))
        self.assertIn("hvc1", (out / MASTER).read_text())
        st = probe(out / "init.mp4")
        self.assertEqual((st["codec_name"], st["codec_tag_string"]), ("hevc", "hvc1"))

    def test_low_latency_mode_uses_half_second_segments(self):
        out = run_for(settings(latency="low"))
        lines = [l for l in (out / "stream.m3u8").read_text().splitlines() if l.startswith("#EXTINF")]
        durations = [float(l.split(":")[1].rstrip(",")) for l in lines]
        self.assertTrue(durations and max(durations) <= 0.6, durations)


if __name__ == "__main__":
    unittest.main()
