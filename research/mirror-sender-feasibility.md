# Windows AirPlay screen-mirroring sender: feasibility and build plan

Status date: 2026-10-09 (America/Los_Angeles)

## Executive conclusion

A Windows-to-AirPlay screen-mirroring sender is feasible, but it is not a
small extension to `pyatv`. A sender must implement receiver discovery,
pairing and pair verification, the FairPlay SAP sender challenge, receiver
capability negotiation, media-key derivation, the AirPlay mirroring packet
format and clocking, and a low-latency H.264 capture/encode path.

The largest historical blocker, generation of the sender-side FairPlay SAP
response, now has two open implementations with test vectors. `doubletake`
also supplies a complete Go sender that has been exercised against several
Apple TV generations and third-party receivers. Its capture path is Linux
specific, but its capture boundary consumes Annex-B access units and is a
good fit for a Windows FFmpeg process.

The proposed first implementation therefore ports, rather than reinvents,
the protocol core from `doubletake` at pinned revision
[`5e2f1cbe`](https://github.com/omarroth/doubletake/tree/5e2f1cbe3e1070bcefec8e549c7baf884727bc00),
preserves its LGPL-3.0-or-later licensing and attribution, and adds a Windows
capture backend that uses FFmpeg's `ddagrab` filter. `ddagrab` is explicitly a
DXGI Desktop Duplication API source. It can select a display by output index
and feed a low-latency H.264 encoder without taking a dependency on the
Linux-only PipeWire/X11 capture stack.

This is technically actionable now. It is not yet evidence that this branch
mirrors successfully to a real receiver; only a hardware run can establish
that.

## What an AirPlay mirroring sender must do

The term "FairPlay" covers two different things that are easy to conflate:

- FairPlay SAP authenticates an AirPlay sender to a receiver. It is part of
  the mirroring session setup and is required even for an ordinary desktop.
- FairPlay Streaming protects commercial media. Decrypting or bypassing
  protected content is not part of this project. Protected windows may be
  blank in a desktop capture.

The sender pipeline is:

1. Browse `_airplay._tcp` with mDNS and parse TXT features/status flags.
2. Fetch `/info` and select the receiver's supported pairing, timing, codec,
   resolution, and stream-encryption modes.
3. If required, perform PIN/password SRP setup and persist the long-term
   Ed25519 credentials securely.
4. Perform pair-verify (X25519, Ed25519, HKDF, and encrypted verification
   records) for the current connection.
5. Exchange FairPlay SAP messages M1 through M4 at `POST /fp-setup`.
6. Negotiate control, event, timing, audio, and type-110 screen-video streams
   through the receiver's RTSP/HTTP-style control connection.
7. Capture one Windows display, encode H.264 with frequent IDR frames, emit
   SPS/PPS plus Annex-B access units, and convert them to AirPlay's codec and
   encrypted video packets.
8. Maintain the receiver clock mapping and tear down all streams cleanly.

Legacy mirroring descriptions use `GET /stream.xml`, `POST /stream`, a
72-byte FairPlay-wrapped AES key (`param1`), a 16-byte IV (`param2`), and the
128-byte mirroring packet header. Modern AirPlay 2 receivers add pairing,
control/event stream setup, capability-dependent timing, and authenticated
stream encryption. A usable sender needs both compatibility branches rather
than assuming one captured handshake fits every receiver.

## Existing work surveyed

### `doubletake`: primary protocol foundation

[`omarroth/doubletake`](https://github.com/omarroth/doubletake) is an
LGPL-3.0-or-later AirPlay screen-mirroring sender written in Go. At pinned
revision `5e2f1cbe3e1070bcefec8e549c7baf884727bc00`, it includes:

- mDNS receiver discovery and `/info` capability parsing;
- PIN/password pairing, pair verification, and credential storage;
- a clean Go FairPlay SAP implementation and vector tests;
- legacy AES and modern ChaCha20-Poly1305 media-key/stream paths;
- control, event, NTP/PTP, screen-video, and audio setup;
- AirPlay packetization for H.264 and HEVC;
- an in-process test receiver for hardware-free protocol tests; and
- documented tests against AppleTV3,2, AppleTV11,1, AppleTV14,1, Macs, Roku,
  Samsung, TCL, and Hisense receivers.

Its capture implementation is Linux-specific (X11 or
PipeWire/xdg-desktop-portal through GStreamer). Importantly, the transport
also supports a plain Annex-B byte stream, so a Windows producer can be added
without changing the AirPlay protocol machinery.

### Portable FairPlay SAP core

[`objevovat/fairplay-sap-core-airplay2-sender-authentication-handshake`](https://github.com/objevovat/fairplay-sap-core-airplay2-sender-authentication-handshake)
documents and implements the M1/M2/M3/M4 sender exchange. The pinned research
revision is `370f9db2e26b21b4a710bdba5d51012c1239e736`.

The useful evidence is unusually specific:

- 142 golden vectors and agreement with independent emulator-based
  implementations;
- real acceptance/rejection tests on three HomePod-family devices; and
- a session-aware M3 construction, which strict receivers require.

Its limits matter just as much:

- it implements FairPlay message mode 3 only;
- its published hardware confirmation covers HomePods, not Apple TVs; and
- some Apple TVs and Macs reject earlier pairing/setup stages, so those tests
  do not validate the SAP response on those products.

The combined implementation is LGPL-3.0-or-later and includes Apple-derived
white-box tables. If used directly, its notice and corresponding-source
requirements must be retained. The `doubletake` import already contains its
own compatible implementation, so the initial build does not add a second SAP
dependency.

### AirSpan: Windows implementation evidence

[`DiznyP/AirSpan`](https://github.com/DiznyP/AirSpan) is a Windows Electron
sender. Pinned revision `a2a032a90c019c235636491dab62db6ee5b0b880`
contains mDNS discovery, PIN pairing, pair verification, FairPlay setup,
control/audio/video setup, WebCodecs H.264 encoding, and AirPlay video packet
generation. It demonstrates that the protocol can be integrated with the
Windows desktop capture stack.

AirSpan is useful corroborating source material, but it is not selected as
the base here. Its protocol is concentrated in one very large JavaScript
module, its committed history is shallow, and its Electron capture handler at
the pinned revision chooses the first enumerated screen. The Go transport plus
a focused native Windows capture seam is easier to test and audit.

### OpenAirPlay protocol notes

The unofficial
[`openairplay/airplay-spec` screen-mirroring notes](https://github.com/openairplay/airplay-spec/tree/master/src/screen_mirroring)
describe the legacy `/stream` request, the 128-byte video packet header,
SPS/PPS handling, AES parameters, and NTP behavior. These notes remain useful
for packet-level orientation but predate modern AirPlay 2 pairing and stream
setup. They are not sufficient by themselves for a current sender.

### Receiver projects and `pyatv`

UxPlay, RPiPlay, and related projects implement the opposite direction: they
make a computer behave as a receiver and therefore unwrap/decrypt sender
traffic. They are valuable for interoperability tests and receiver-side
packet interpretation, but they do not provide the sender authentication and
key-wrapping flow as a drop-in client.

`pyatv` is valuable for discovery, control, metadata, and existing AirPlay
features, but it does not expose a desktop-mirroring sender that performs SAP,
media key establishment, and the mirroring video stream. Calling `pyatv` is
therefore not a substitute for the pipeline above.

## Windows capture and encoding decision

The initial Windows backend will launch FFmpeg and use:

- `ddagrab=output_idx=N` for a selected DXGI output;
- `draw_mouse=true|false` for cursor policy;
- D3D11 hardware-frame download where a software encoder is used;
- aspect-preserving scale/pad to the receiver's negotiated even dimensions;
- H.264 with zero B-frames, periodic IDR frames, AUD, repeated headers, and
  zero-latency tuning; and
- Annex-B output over stdout into the existing sender transport.

FFmpeg 9.0.2 on the development host advertises the `ddagrab` filter and
`libx264`, `h264_mf`, `h264_nvenc`, `h264_qsv`, and `h264_amf` encoders. The
portable baseline will be `libx264`; hardware encoders can be probed and added
after the packet flow is stable. This keeps correctness independent of a
particular GPU vendor while still ensuring capture itself uses Desktop
Duplication.

Display enumeration will use Win32 display APIs and expose stable selection
by the same active-output ordering used by `ddagrab`. The CLI will support
listing displays, an explicit display index, and an interactive picker.

## Concrete implementation plan

### Milestone 1: preserve and verify the protocol core

- Import the pinned `doubletake` source with its LGPL/GPL license texts.
- Add an upstream/provenance notice and rename the user-facing binary to
  `open-airplay` without obscuring the source origin.
- Make process supervision compile on Windows without weakening the Linux
  parent-death behavior.
- Run its unit and in-process receiver tests on Windows.

Exit criterion: `go test ./...` and `go build ./cmd/open-airplay` pass on
Windows, independent of an AirPlay receiver.

### Milestone 2: selected-display capture and H.264

- Enumerate active monitors through Win32.
- Add `--list-displays` and `--display` selection.
- Add the `ddagrab`/H.264 FFmpeg backend and startup validation.
- Unit-test command construction, dimension handling, and selection.
- Add a capture smoke mode that reads SPS, PPS, and an IDR without connecting
  to a receiver.

Exit criterion: a selected physical display produces parseable H.264
SPS/PPS/IDR access units on the development machine.

### Milestone 3: AirPlay session integration

- Feed the capture into the existing setup/packetization path.
- Default Windows sessions to video-only until WASAPI loopback audio is
  implemented; do not advertise or silently send an unsupported audio path.
- Validate discovery and pairing error messages without logging PINs, keys,
  or persistent credentials.

Exit criterion: the in-process receiver accepts setup and receives video
packets from the Windows backend.

### Milestone 4: real receiver validation

Test at least one current Apple TV and, if available, one third-party AirPlay
2 receiver. Record, per model and firmware:

- discovery and `/info`;
- initial PIN pairing and saved-credential reconnect;
- SAP mode selected and result;
- timing mode, negotiated canvas, and encryption mode;
- first SPS/PPS/IDR acceptance;
- ten-minute stream stability, cursor, display choice, and teardown; and
- measured capture-to-display latency.

Exit criterion: a real receiver visibly mirrors the selected Windows display.
Until this is performed, the project must say "hardware unverified," even if
all automated and local capture tests pass.

## Known risks and honest blockers

- Receiver behavior varies by model and firmware. FairPlay mode 3 is the only
  mode implemented by the open sender cores surveyed.
- Apple TV failures can occur in pairing or control setup before SAP is even
  evaluated; successful HomePod SAP evidence cannot be generalized to every
  Apple TV.
- Legacy AES-wrapped mirroring and current AirPlay 2 stream encryption require
  separate compatibility branches and accurate capability selection.
- Capture output indices can change after docking, GPU reset, or topology
  changes. The picker must show device names and geometry every run instead of
  persisting an unexplained integer forever.
- HDR desktops need an explicit SDR conversion policy for broadly compatible
  H.264 output.
- Screen audio is a separate RTP/timing problem. It is deliberately outside
  the first video milestone.
- No real AirPlay receiver is attached to this build environment at the time
  of writing. End-to-end status is therefore undetermined, not passed.

## Legal and security notes

- Preserve `doubletake`'s LGPL-3.0-or-later terms, incorporated GPL text, and
  upstream provenance in source and distributions.
- Do not market this as FairPlay Streaming DRM support and do not attempt to
  capture protected video surfaces.
- Pairing records and long-term keys are secrets. Windows builds should use
  Credential Manager where available, restrict any file fallback to the user,
  and redact keys, PINs, passwords, and device identifiers from diagnostics.
- Network parsing and receiver capability data are untrusted input. Retain
  bounds checks from the protocol core and fuzz the plist and frame parsers as
  the Windows port evolves.
