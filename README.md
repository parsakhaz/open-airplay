# open-airplay

An open-source AirPlay screen-mirroring sender, starting with Windows.

## Mirror sender status

This branch contains a complete AirPlay sender protocol foundation: discovery,
PIN/password pairing, pair verification, FairPlay SAP, receiver capability
negotiation, clocking, encrypted H.264 packetization, and an in-process test
receiver. The protocol code is based on a pinned revision of
[`omarroth/doubletake`](https://github.com/omarroth/doubletake); see
[NOTICE.md](NOTICE.md) and the included LGPL/GPL license texts.

The Windows sender now enumerates displays, captures the selected display with
FFmpeg's DXGI Desktop Duplication source, and produces a low-latency H.264
Annex-B stream. A local integration fixture has exercised pairing, FairPlay,
encrypted setup, RECORD, feedback, video transport, and teardown.

On October 10, 2026, an owner-run session also completed pairing and FairPlay
setup with a physical Apple TV 4K (`AppleTV14,1`) and displayed the real Windows
desktop. The owner described latency as near-instant; that is a qualitative
observation, not an instrumented measurement. This first confirmed hardware
run used the previous automatic profile: `1280x720` at `30 fps` and `1843
kbps`. It validates one receiver model and configuration, not every Apple TV,
firmware version, or third-party implementation. See
[the validation report](research/mirror-sender-validation.md) for the exact
evidence and remaining limits, and
[the feasibility report](research/mirror-sender-feasibility.md) for the
protocol survey and risk analysis.

## Mirror your Windows desktop

The Windows PC and receiver must be on the same local network. Install Go 1.25
or newer and put an FFmpeg build with `ddagrab` and `libx264` on `PATH`.

Discovery is read-only: it does not connect, pair, capture, or start mirroring.

```console
go run ./cmd/open-airplay -list-receivers
go run ./cmd/open-airplay -list-displays
```

The receiver list includes every discovered AirPlay advertisement with a
one-based index, name, model, endpoint, and `mirroring=yes` or `mirroring=no`.
Only select a receiver that belongs to you. Never test against a conference
room, office, or otherwise shared display.

For a first-time PIN pairing, use the exact receiver name and chosen display:

```console
go run ./cmd/open-airplay -receiver "MY APPLE TV NAME" -display 0 -pair
```

When the Apple TV uses on-screen PIN pairing, it displays a PIN and the
terminal names the selected receiver in a prominent `PAIRING REQUIRED` prompt;
type the PIN there and press Enter. A password-configured receiver prompts for
that password instead, and a receiver that accepts transient pairing may not
show a PIN. Credentials are then saved in Windows Credential Manager. On later
runs, omit `-pair` to reuse them:

```console
go run ./cmd/open-airplay -receiver "MY APPLE TV NAME" -display 0
```

With `-fps 0` and `-bitrate 0` (the defaults), the sender resolves the final
profile after it receives the Apple TV's session-time display capabilities. It
prefers the selected Windows display's native resolution without upscaling
when the receiver's advertised decoder limit and the low-latency H.264 ceiling
allow it. Larger sources are aspect-fitted to those limits, with H.264 capped
at `1920x1080`; without usable maximum-size metadata, a source that fits the
nominal canvas keeps its native size and a larger source retains the nominal
canvas. Frame rate uses the receiver-advertised maximum up to `60 fps`, or a
conservative `30 fps` when that metadata is absent. Automatic bitrate is then
derived from the resolved resolution and frame rate.

On the `1440x900` validation display, a receiver advertising at least that
canvas and `60 fps` now resolves to `1440x900` at `60 fps` and `5184 kbps`.
That revised default still needs a physical-device regression run. Positive
`-fps` and `-bitrate` values remain explicit overrides; an FPS above the
receiver-advertised maximum is rejected. Existing `-video-codec` and
`-hwaccel` overrides are unchanged.

You can instead pass the one-based index from the immediately preceding
discovery result, for example `-receiver 2`. Indices can change when devices
join or leave the network, so the exact name is safer. An all-numeric
`-receiver` value is always interpreted as an index. Duplicate names,
non-mirroring receivers, missing selections, and ambiguous selections are
rejected. The CLI prints the chosen name, model, and endpoint before it
connects. Press Ctrl+C to stop mirroring and send teardown.

Windows mirroring is currently video-only because a WASAPI capture path has
not been implemented. Omit `-display` to choose interactively when more than
one monitor is active. PINs and receiver passwords can alternatively be
supplied through `OPEN_AIRPLAY_CODE` so they do not enter shell history or the
process list.

## Product direction

The planned product is one application whose default transport is this direct
FairPlay sender, with streaming/HLS used as an automatic compatibility
fallback. The settings toggle and fallback are intentionally outside this
change. Discovery, capture, pairing, FairPlay, and transport remain in the
`internal/airplay` package. Its `StreamConfig` accepts source dimensions and
its profile-aware setup hook passes the final canvas, FPS, and codec to an
embedding caller, so the protocol and capture path do not depend on CLI flag
parsing. Before GUI integration, the remaining foreground orchestration should
move into a small reusable sender facade so the CLI and application invoke
exactly the same complete lifecycle. A fallback must retain the exact
user-selected receiver and must not trigger for a wrong PIN, user cancellation,
or ambiguous target.

## Remaining work and ETA

The direct video path is proven on one physical Apple TV, but a polished
Windows preview still needs:

- a real-device regression of the new native-resolution/automatic-FPS profile,
  saved-credential reconnect, teardown, and a 30-60 minute soak;
- WASAPI loopback capture, audio transport, A/V synchronization, and drift
  testing;
- Windows packaging, signing, and an FFmpeg dependency/distribution strategy;
- validation that exact by-name receiver selection and its failure UX work from
  the packaged, shipped build, not only `go run` during development; and
- first-pass mixed-DPI, docking, rotation, HDR, GPU, and interrupted-network
  rough-edge coverage.

Assuming no receiver-firmware, audio-driver, or packaging blockers, the
estimate for that polished Windows preview is **7-10 business days**, targeting
**October 20-23, 2026**.

## Development

The project requires Go 1.25 or newer.

```console
go test ./...
go build ./cmd/open-airplay
go run ./cmd/open-airplay -list-receivers
go run ./cmd/open-airplay -list-displays
go run ./cmd/open-airplay -display 0 -capture-smoke
go run ./cmd/open-airplay -receiver "MY APPLE TV NAME" -display 0 -pair
```

On Windows, capture uses FFmpeg's `ddagrab` source (the DXGI Desktop
Duplication API) and emits low-latency Annex-B H.264. FFmpeg must be available
on `PATH`. Omit `-display` to choose interactively when more than one monitor
is active. The smoke command validates capture and encoding without contacting
an AirPlay receiver. Audio is disabled on Windows because a WASAPI capture path
has not been implemented yet. Picker indices are native DXGI output indices on
FFmpeg's default D3D11 adapter; outputs attached only to other adapters are not
addressable by the current `ddagrab` path.

Pairing credentials default to Windows Credential Manager on Windows.

Do not interpret successful unit tests as broad real-device validation. The
confirmed hardware result covers one `AppleTV14,1` session; receiver firmware,
pairing mode, negotiated profile, timing, visible acceptance, duration, and
teardown should continue to be recorded during hardware testing.
