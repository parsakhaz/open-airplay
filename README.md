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
encrypted setup, RECORD, feedback, video transport, and teardown. End-to-end
mirroring has **not** been verified against physical Apple hardware from this
branch. See [the validation report](research/mirror-sender-validation.md) for
the exact evidence and remaining limits, and
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
`internal/airplay` package. Before GUI integration, the remaining command
orchestration should move into a small reusable sender facade so the CLI and
application invoke exactly the same session flow. A fallback must retain the
exact user-selected receiver and must not trigger for a wrong PIN, user
cancellation, or ambiguous target.

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

Do not interpret successful unit tests as real-device validation. Receiver
model, firmware, pairing mode, timing mode, and video acceptance must be
recorded separately during hardware testing.
