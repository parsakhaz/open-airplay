# screencast: a Windows display on an Apple TV, as live HLS

This puts a Windows display on an Apple TV without AirPlay screen mirroring. Mirroring needs FairPlay; this doesn't.
It captures the display, encodes it with ffmpeg and serves it from this PC as a live HLS stream. Then it tells the Apple TV to play that stream through AirPlay's video path.
It works, but it is a live stream, not a mirror: it runs **seconds behind the screen**.

## Run it

You need Python 3.12, ffmpeg on PATH, and `pip install -r screencast/requirements.txt` (pyatv 0.18.0, pinned).

```
python -m screencast                                          # control UI at http://127.0.0.1:7111/
python -m screencast cast --receiver "Living Room"            # cast the main display until Ctrl+C
python -m screencast cast --receiver "Living Room" --display 1 --latency low --seconds 600
python -m screencast caps                                     # what this PC and the LAN's receivers can do
python -m screencast.marker                                   # on-top clock, to check what the TV shows and how late
```

**First cast to an Apple TV:** tvOS asks for a one-time pairing. Start the cast from the UI, then type the 4-digit code the TV shows. The credential is saved in `%LOCALAPPDATA%\open-airplay\`, so later casts and the CLI just work.
**Firewall:** the TV fetches the stream from this PC on port 7111. The UI's Setup panel checks for a rule and can request one through Windows' own prompt.

## Measured latency

Measured on an Apple TV 4K (gen 3) on tvOS 26.6, over Wi-Fi, with a 2880×1800 HEVC stream at 60 fps and about 21 Mb/s.
We read the on-screen clock against the TV by eye.

| Mode | How | Measured delay |
| --- | --- | --- |
| `balanced` (default) | 1 s segments | **3.5 to 5 s, steady** |
| `low` | 0.5 s segments; the playlist lists the next 2 segments before they exist, and the server holds those requests until each is encoded | **about 1.5 to 2.5 s at start, drifting to about 3 s** after stalls |
| `quality` | 2 s segments, B-frames | not measured |

**Why it can't go lower here:** the Apple TV's player starts at least 3 target durations behind the newest listed segment, and HLS can't advertise a target duration under 1 s.
The player also never catches up after a stall.
Apple's Low-Latency HLS might reach about 1.5 to 2 s, but it needs HTTP/2 over HTTPS with a certificate the TV trusts, and that isn't built.
Near-real-time needs real mirroring, or an app on the TV.
