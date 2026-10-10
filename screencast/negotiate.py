"""Pick the best settings this machine + receiver pair can actually do, and the options we may offer.

Pure: no I/O. The UI only ever shows `Plan.options`, so it never presents something either side can't do.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass, replace

from .caps import Display, Encoder, MachineCaps, ReceiverCaps

LATENCY = {  # HLS segment seconds per mode; AVPlayer holds roughly 3 segments behind live
    "low": {"label": "Low latency (~1.5-2.5 s measured, can drift to ~3 s)", "segment": 0.5, "window": 6, "ahead": 2},
    "balanced": {"label": "Balanced (~3.5-5 s measured, steady)", "segment": 1.0, "window": 6, "ahead": 0},
    "quality": {"label": "Quality (2 s segments, B-frames; ~6-7 s estimated, not measured)", "segment": 2.0, "window": 6, "ahead": 0},
}
_BPP = {"hevc": 0.07, "h264": 0.11}  # bits per pixel per frame that keeps desktop text crisp
_BITRATE_TIERS = (("max", "Maximum", 1.6), ("high", "High (recommended)", 1.0),
                  ("balanced", "Balanced", 0.6), ("low", "Low / weak Wi-Fi", 0.35))
_FPS = (60, 50, 30, 24)
_SCALES = (100, 75, 50)
CODEC_LABEL = {"hevc": "HEVC (H.265)", "h264": "H.264"}


@dataclass(frozen=True)
class Settings:
    codec: str
    encoder: str
    width: int
    height: int
    fps: int
    bitrate_kbps: int
    latency: str
    audio: bool
    scale: int = 100
    audio_source: str | None = None


@dataclass(frozen=True)
class Plan:
    defaults: Settings
    options: dict
    notes: tuple

    def to_json(self):
        return {"defaults": asdict(self.defaults), "options": self.options, "notes": list(self.notes)}


def _even(v):
    return max(2, int(v) // 2 * 2)


def _fit(display: Display, rx: ReceiverCaps, enc: Encoder, scale=100):
    f = min(1.0, rx.max_width / display.width, rx.max_height / display.height, enc.max_height / display.height)
    f *= scale / 100
    return _even(display.width * f), _even(display.height * f)


def _fps_cap(display, rx, enc):
    return max(1, min(display.refresh or 60, rx.max_fps, enc.max_fps))


def default_bitrate(codec, w, h, fps):
    return int(min(80000, max(4000, w * h * fps * _BPP[codec] / 1000)))


def _rank(display, rx, enc):
    w, h = _fit(display, rx, enc)
    return (w * h * _fps_cap(display, rx, enc), enc.codec == "hevc", enc.hardware)


def negotiate(machine: MachineCaps, rx: ReceiverCaps, display: Display, encoder: str | None = None) -> Plan:
    if not rx.video:
        raise ValueError(f"{rx.name or rx.model} is an audio-only AirPlay receiver; it cannot play video")
    codecs = {"h264", "hevc"} if rx.hevc else {"h264"}
    usable = [e for e in machine.encoders if e.codec in codecs]
    if not usable:
        raise ValueError("no working H.264/HEVC encoder on this machine (is ffmpeg installed?)")
    best = max(usable, key=lambda e: _rank(display, rx, e))
    enc = next((e for e in usable if e.name == encoder), best) if encoder else best

    notes = list(machine.notes)
    if not rx.hevc:
        notes.append(f"Receiver {rx.source} is not known to decode HEVC: using H.264.")
    elif not any(e.codec == "hevc" for e in machine.encoders):
        notes.append("No HEVC encoder on this machine: using H.264.")
    elif best.codec != "hevc":
        notes.append("HEVC is only available as a slow software encoder here: H.264 gives more resolution.")
    if not any(e.hardware for e in usable):
        notes.append("No hardware encoder found (NVENC/QSV/AMF): using software encoding, limited to "
                     f"{best.max_height}p{best.max_fps} on this CPU.")
    if rx.hdr:
        notes.append("Receiver supports HDR, but HDR passthrough is not implemented yet: streaming SDR.")

    w, h = _fit(display, rx, enc)
    cap = _fps_cap(display, rx, enc)
    fps_opts = [f for f in _FPS if f <= cap] or [cap]
    base = default_bitrate(enc.codec, w, h, fps_opts[0])
    audio = machine.audio_sources

    options = {
        "codec": [{"value": c, "label": CODEC_LABEL[c]} for c in ("hevc", "h264")
                  if any(e.codec == c for e in usable)],
        "encoder": [{"value": e.name, "codec": e.codec,
                     "label": f"{e.name} ({'hardware' if e.hardware else 'software'}, up to {e.max_height}p{e.max_fps})"}
                    for e in usable],
        "scale": [{"value": s, "label": "{}×{}{}".format(*_fit(display, rx, enc, s), " (full)" if s == 100 else "")}
                  for s in _SCALES],
        "fps": [{"value": f, "label": f"{f} fps"} for f in fps_opts],
        "bitrate_kbps": [{"value": int(min(80000, base * m)), "label": f"{label}: {min(80000, base * m) / 1000:.0f} Mb/s"}
                         for _, label, m in _BITRATE_TIERS],
        "latency": [{"value": k, "label": v["label"]} for k, v in LATENCY.items()],
    }
    if audio:
        options["audio"] = [{"value": True, "label": f"On ({audio[0].name})"}, {"value": False, "label": "Off"}]

    defaults = Settings(codec=enc.codec, encoder=enc.name, width=w, height=h, fps=fps_opts[0],
                        bitrate_kbps=base, latency="balanced", audio=bool(audio),
                        audio_source=audio[0].name if audio else None)
    return Plan(defaults=defaults, options=options, notes=tuple(notes))


def apply_overrides(plan: Plan, machine: MachineCaps, rx: ReceiverCaps, display: Display, overrides: dict) -> Settings:
    """Apply the user's advanced choices, rejecting anything outside the plan's options."""
    s = plan.defaults
    allowed = {k: [o["value"] for o in v] for k, v in plan.options.items()}
    for key in ("fps", "latency", "audio", "scale", "bitrate_kbps"):
        val = overrides.get(key)
        if val is None:
            continue
        if key == "bitrate_kbps":  # any value in a sane range, the tiers are suggestions
            val = int(val)
            if not 1000 <= val <= 120000:
                raise ValueError("bitrate must be between 1 and 120 Mb/s")
        elif val not in allowed.get(key, []):
            raise ValueError(f"{key}={val!r} is not supported by this machine/receiver (allowed: {allowed.get(key)})")
        s = replace(s, **{key: val})
    if s.scale != 100:
        enc = next(e for e in machine.encoders if e.name == s.encoder)
        s = replace(s, **dict(zip(("width", "height"), _fit(display, rx, enc, s.scale))))
    if not s.audio:
        s = replace(s, audio_source=None)
    return s
