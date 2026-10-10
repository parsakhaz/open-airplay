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

## Development

The project requires Go 1.25 or newer.

```console
go test ./...
go build ./cmd/open-airplay
go run ./cmd/open-airplay -list-receivers
go run ./cmd/open-airplay -list-displays
go run ./cmd/open-airplay -display 0 -capture-smoke
go run ./cmd/open-airplay -display 0
```

On Windows, capture uses FFmpeg's `ddagrab` source (the DXGI Desktop
Duplication API) and emits low-latency Annex-B H.264. FFmpeg must be available
on `PATH`. Omit `-display` to choose interactively when more than one monitor
is active. The smoke command validates capture and encoding without contacting
an AirPlay receiver. Audio is disabled on Windows because a WASAPI capture path
has not been implemented yet. Picker indices are native DXGI output indices on
FFmpeg's default D3D11 adapter; outputs attached only to other adapters are not
addressable by the current `ddagrab` path.

Pairing credentials default to Windows Credential Manager on Windows. Supply a
PIN or receiver password through `OPEN_AIRPLAY_CODE` to keep it out of shell
history and process listings.

Do not interpret successful unit tests as real-device validation. Receiver
model, firmware, pairing mode, timing mode, and video acceptance must be
recorded separately during hardware testing.
