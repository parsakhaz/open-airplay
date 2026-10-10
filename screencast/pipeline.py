"""ffmpeg command for: capture a display (Desktop Duplication) -> scale/convert -> H.264/HEVC -> live HLS."""
from __future__ import annotations

from .caps import FFMPEG, Display
from .negotiate import LATENCY, Settings

MASTER = "master.m3u8"


def capture_input(display: Display, fps: int, region: tuple | None = None) -> list:
    """ddagrab input for one display (or a region of it, in that display's physical pixels)."""
    opts = [f"output_idx={display.index}", f"framerate={fps}", "draw_mouse=1"]
    if region:
        x, y, w, h = region
        opts += [f"offset_x={x}", f"offset_y={y}", f"video_size={w - w % 2}x{h - h % 2}"]
    # Frames arrive as D3D11 textures; download to BGRA so any encoder (hw or sw) can take them.
    return ["-f", "lavfi", "-i", f"ddagrab={':'.join(opts)},hwdownload,format=bgra"]


def _encoder_args(s: Settings, gop: int) -> list:
    b, quality = s.bitrate_kbps, s.latency == "quality"
    rate = ["-b:v", f"{b}k", "-maxrate", f"{int(b * 1.5)}k", "-bufsize", f"{b}k"]
    family = s.encoder.split("_")[-1]
    if family == "amf":
        args = ["-usage", "transcoding" if quality else "lowlatency", "-quality", "quality", "-rc", "vbr_peak"]
    elif family == "nvenc":
        args = ["-preset", "p5", "-tune", "hq" if quality else "ll", "-rc", "vbr", "-bf", "2" if quality else "0"]
    elif family == "qsv":
        args = ["-preset", "medium", "-look_ahead", "0", "-bf", "2" if quality else "0"]
    elif s.encoder == "libx264":
        args = ["-preset", "faster"] if quality else ["-preset", "veryfast", "-tune", "zerolatency"]
    elif s.encoder == "libx265":
        args = ["-preset", "ultrafast", "-x265-params", "log-level=error"] + ([] if quality else ["-tune", "zerolatency"])
    else:
        args = []
    profile = ["-profile:v", "main"] if s.codec == "hevc" else ["-profile:v", "high"]
    return ["-c:v", s.encoder, *args, *rate, *profile, "-g", str(gop), "-keyint_min", str(gop)]


def build_args(s: Settings, video_input: list, audio_input: list | None = None) -> list:
    """Arguments after `ffmpeg`. Run it with cwd set to the output directory: it writes
    master.m3u8 -> stream.m3u8 -> segments there (ffmpeg places the fMP4 init file relative to cwd)."""
    seg = LATENCY[s.latency]["segment"]
    gop = max(1, round(s.fps * seg))  # one IDR per segment so every segment starts independently
    pix = "nv12" if s.encoder.split("_")[-1] in ("amf", "nvenc", "qsv") else "yuv420p"
    # RGB -> BT.709 limited range explicitly; swscale would default to BT.601 and shift colors.
    vf = f"scale={s.width}:{s.height}:flags=lanczos:out_color_matrix=bt709:out_range=tv,format={pix}"
    fmp4 = s.codec == "hevc"  # Apple's HLS spec requires fMP4 for HEVC; TS is the most compatible for H.264
    args = ["-fflags", "nobuffer", "-thread_queue_size", "64", *video_input]
    if s.audio and audio_input:
        args += audio_input
    args += ["-map", "0:v"] + (["-map", "1:a", "-c:a", "aac", "-b:a", "192k", "-ar", "48000"] if s.audio and audio_input else [])
    args += ["-vf", vf, *_encoder_args(s, gop),
             "-force_key_frames", f"expr:gte(t,n_forced*{seg})",
             "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"]
    if s.codec == "hevc":
        args += ["-tag:v", "hvc1"]
    args += ["-f", "hls", "-hls_time", str(seg), "-hls_list_size", str(LATENCY[s.latency]["window"]), "-hls_delete_threshold", "4",
             "-hls_flags", "delete_segments+independent_segments+omit_endlist+temp_file",
             "-hls_segment_type", "fmp4" if fmp4 else "mpegts",
             "-hls_segment_filename", "seg_%05d.m4s" if fmp4 else "seg_%05d.ts",
             "-master_pl_name", MASTER]
    if fmp4:
        args += ["-hls_fmp4_init_filename", "init.mp4"]
    return args + ["stream.m3u8"]


def ffmpeg_command(s: Settings, display: Display, region=None) -> list:
    audio = ["-f", "dshow", "-i", f"audio={s.audio_source}"] if s.audio and s.audio_source else None
    return [FFMPEG, "-hide_banner", "-loglevel", "warning", "-nostats", "-progress", "pipe:1",
            *build_args(s, capture_input(display, s.fps, region), audio)]
