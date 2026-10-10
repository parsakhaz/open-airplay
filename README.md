# open-airplay

**AirPlay for Windows — done right.** Receive AirPlay from your iPhone/Mac (screen mirroring, audio, video) and
mirror your Windows desktop to an Apple TV with real, low-latency FairPlay mirroring (not a laggy HLS stream).

> **Bonus:** this is part of a single "Apple interop" app that also does **[AirDrop on Windows](https://github.com/parsakhaz/open-airdrop-windows)** — send and receive files with iPhones and Macs.

Clean, Apple-like UI. Open source (GPL-3.0). Windows first.

Status: in active development — AirPlay receiver and FairPlay desktop→Apple TV mirror both working and verified on real hardware. See the open pull requests for progress.

## Credits & acknowledgments

open-airplay exists because many people spent years taking AirPlay and Apple's wireless protocols apart and
publishing what they learned. Thank you. A visual history is in **[docs/timeline.html](docs/timeline.html)**
(download it and open it in any browser).

**AirPlay mirroring, sender side**
- **[doubletake](https://github.com/omarroth/doubletake)** by [Omar Roth](https://omar.yt) — the AirPlay
  screen-mirroring *sender* (FairPlay SAP authentication, pairing, encrypted video stream) that our Windows → Apple TV
  mirroring is ported from. Without it, real FairPlay mirroring from Windows would not exist in this project.
  LGPL-3.0-or-later.
- **[pyatv](https://github.com/postlund/pyatv)** by Pierre Ståhl ([pyatv.dev](https://pyatv.dev)) — the Python
  client library for Apple TV and AirPlay devices. MIT.

**AirPlay receiver lineage**
- **[shairport](https://github.com/abrasive/shairport)** by James Laird (2011) — the open AirTunes (RAOP) receiver,
  built on keys recovered from an AirPort Express, that opened AirPlay audio to non-Apple hardware.
- **[shairport-sync](https://github.com/mikebrady/shairport-sync)** by Mike Brady — shairport's long-lived
  successor, with multi-room sync and, since 4.1, AirPlay 2.
- **[ShairPlay](https://github.com/juhovh/shairplay)** by Juho Vähä-Herttua and contributors — the AirPlay/RAOP
  server library much of the later mirroring work grew from.
- **dsafa22**'s AirplayServer — the Android AirPlay mirroring server that RPiPlay ports (its original repository is
  no longer online).
- **[playfair](https://github.com/EstebanKubata/playfair)** by EstebanKubata — the FairPlay-handling library used
  by RPiPlay and UxPlay. GPL-3.0.
- **[RPiPlay](https://github.com/FD-/RPiPlay)** by FD- — the open AirPlay mirroring server for the Raspberry Pi,
  with an unusually generous write-up of how the protocol evolved. GPL-3.0.
- **[UxPlay](https://github.com/FDH2/UxPlay)**, started by [antimof](https://github.com/antimof/UxPlay) from RPiPlay
  and developed since 2021 by FDH2 — the AirPlay mirroring and audio receiver our receiver engine is built on.
  GPL-3.0.
- **[Unofficial AirPlay Specification](https://openairplay.github.io/airplay-spec/)** (openairplay, continuing
  [nto's original](https://nto.github.io/AirPlay.html)) — the community protocol reference.

**Apple Wireless Direct Link (AWDL) research**
- **[SEEMOO Lab, TU Darmstadt](https://www.seemoo.tu-darmstadt.de/)** — Milan Stute, David Kreitschmann, Alexander
  Heinrich, Matthias Hollick and colleagues, whose
  [*One Billion Apples' Secret Sauce*](https://arxiv.org/abs/1808.03156) (MobiCom '18) documented AWDL, the link
  under AirPlay and AirDrop, and whose open implementations **[OWL](https://github.com/seemoo-lab/owl)** and
  **[OpenDrop](https://github.com/seemoo-lab/opendrop)** made AirDrop possible outside Apple's platforms. See also
  [*A Billion Open Interfaces for Eve and Mallory*](https://www.usenix.org/conference/usenixsecurity19/presentation/stute)
  (USENIX Security '19). (The project's former website, owlink.org, is no longer associated with it, so we don't
  link it.)
- **[omdrop-owl](https://github.com/t4t5/omdrop-owl)** by [Tristan (t4t5)](https://t4t5.com) — AirDrop on a
  MediaTek MT7925 over OWL while Wi-Fi stays connected, which inspired the AirDrop half of this app
  ([open-airdrop-windows](https://github.com/parsakhaz/open-airdrop-windows)). It builds on
  [omdrop](https://github.com/brentkearney/omdrop-plugin) and
  [omdrop-awdl](https://github.com/brentkearney/omdrop-awdl) by Brent (brentkearney) and
  [jedbillyb's OWL fork](https://github.com/jedbillyb/owl).

### Licenses, in plain language

open-airplay is licensed under the **GNU GPL-3.0**. The projects whose code we build on are GPL-compatible:
UxPlay, RPiPlay, playfair, OWL and OpenDrop are **GPL-3.0**; doubletake is **LGPL-3.0-or-later**, which is GPL-3.0
plus extra permissions, so it combines cleanly with a GPL-3.0 app. Code we port keeps its original copyright and
license notices, and each component's `NOTICE` file records what came from where.
pyatv is MIT. This is a summary, not legal advice; the license files are what count.

### A note on FairPlay provenance

doubletake's FairPlay SAP code contains constants and tables needed to talk to Apple receivers. In
[doubletake issue #59](https://github.com/omarroth/doubletake/issues/59) (October 2026) a user asked how they were
derived. The author answered that an earlier version ran a carved-out piece of Apple's own FairPlay code under an
ARM64 emulator, and that the current version (since
[#14](https://github.com/omarroth/doubletake/pull/14), July 2026) is a reverse-engineered implementation containing
no Apple code other than the constants required to interoperate with first-party receivers. We port only that
current implementation. We haven't independently audited this, and we'd rather say so than leave it out. FairPlay
SAP authenticates a sender to a receiver; it is not FairPlay Streaming DRM and does not decrypt protected media.
