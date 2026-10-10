<div align="center">

# open-airplay

### AirPlay for Windows — done right.

Receive AirPlay from your iPhone, iPad, and Mac, and mirror your Windows desktop **to** an Apple TV with real, near‑instant FairPlay mirroring — not a laggy video stream.

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows%2010%2F11-0078D6?logo=windows)](https://github.com/parsakhaz/open-airplay)
![Status: active development](https://img.shields.io/badge/status-active%20development-success)
![Built with Rust + Tauri + Go](https://img.shields.io/badge/built%20with-Rust%20%C2%B7%20Tauri%20%C2%B7%20Go-informational)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](https://github.com/parsakhaz/open-airplay/pulls)

> **Bonus:** part of a single "Apple interop" app that also does **[AirDrop on Windows](https://github.com/parsakhaz/open-airdrop-windows)**.

</div>

---

## What it does

| | Direction | Status |
|---|---|---|
| 📥 **Receive** | iPhone / iPad / Mac → your PC | Screen mirroring, audio, and video. **Working, verified on-device.** |
| 📤 **Mirror (send)** | your Windows desktop → Apple TV | Real FairPlay screen mirroring, **near‑instant** latency. **Working, verified on an Apple TV 4K.** |
| 🎞️ **Cast media** | your PC → Apple TV / AirPlay | Send a video URL or audio file via pyatv. Working. |

The headline feature is **sender‑side FairPlay mirroring**: pushing your live desktop to an Apple TV in real time. This is the piece open source had never cracked on Windows — only paid tools (AirParrot, AirMyPC) did it, because it needs Apple's FairPlay authentication handshake. open-airplay does it in the open.

## Quick start

> Requires Windows 10/11, [Go 1.25+](https://go.dev/dl/), and an [FFmpeg](https://www.gyan.dev/ffmpeg/builds/) build with `ddagrab` on your `PATH`. The PC and the receiver must be on the same network.

**Mirror your desktop to an Apple TV**

```console
# See the AirPlay devices on your network
go run ./cmd/open-airplay -list-receivers

# First time: pair (the Apple TV shows a PIN; type it when prompted)
go run ./cmd/open-airplay -receiver "Living Room" -display 0 -pair

# After that, just:
go run ./cmd/open-airplay -receiver "Living Room" -display 0
```

Credentials are saved in Windows Credential Manager, so you only pair once. Press `Ctrl+C` to stop and tear down cleanly. Only ever target a receiver you own.

**Receive AirPlay on your PC** — launch the desktop app:

```console
npm install && npm run dev
```

Then pick your PC from Screen Mirroring / AirPlay on your iPhone or Mac.

## How it works

- **Receiver** — a supervised [UxPlay](https://github.com/antimof/UxPlay) engine with built‑in device discovery, wrapped in a clean Tauri app (a compact tray popover that expands to a full‑screen media view).
- **Sender (mirroring)** — captures the chosen display via the Windows Desktop Duplication API, encodes H.264 live, performs the AirPlay 2 + FairPlay SAP pairing and handshake, and streams the encrypted mirror to the receiver in real time. The protocol core is ported from [doubletake](https://github.com/omarroth/doubletake).
- **Sender (media)** — hands a video/audio URL to the receiver's AirPlay playback via [pyatv](https://github.com/postlund/pyatv).

Building from source, the installer, the design system and the test suites are covered in **[docs/DEVELOPING.md](docs/DEVELOPING.md)**.

## Credits & acknowledgments

This stands on years of open reverse‑engineering by others. See the **[reverse‑engineering timeline](docs/timeline.html)** and the credits section below for first‑party sources — including the SEEMOO Lab (TU Darmstadt) AWDL/AirPlay research, shairport, RPiPlay/UxPlay, pyatv, and especially **[doubletake](https://github.com/omarroth/doubletake)**, whose FairPlay work makes the mirroring sender possible. Licensed under **GPL‑3.0**; we build on doubletake (LGPL‑3.0) and UxPlay/RPiPlay (GPL‑3.0).

## What's next

- [ ] **Audio for mirroring** — WASAPI capture so the desktop mirror carries sound (currently video‑only).
- [ ] **Higher fidelity by default** — native‑resolution capture, 60fps, hardware encoding (NVENC/QSV/AMF), and adaptive bitrate tuned to the link.
- [ ] **One‑click UI for the mirror** — device picker, PIN flow, and a quality/latency settings panel, folded into the same app as the receiver.
- [ ] **FairPlay provenance review** — document and isolate the FairPlay constants (see doubletake [#59](https://github.com/omarroth/doubletake/issues/59)) before any binary release.
- [ ] **Packaging** — a signed installer and tray app.
- [ ] **AirDrop (bonus)** — the sibling [open-airdrop-windows](https://github.com/parsakhaz/open-airdrop-windows) project; a working path exists via a USB Wi‑Fi dongle in WSL2.

## License

[GPL‑3.0](LICENSE). Not affiliated with or endorsed by Apple. "AirPlay" and "Apple TV" are trademarks of Apple Inc.
