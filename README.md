# open-airplay

An open-source AirPlay screen-mirroring sender, starting with Windows.

## Mirror sender status

This branch contains a complete AirPlay sender protocol foundation: discovery,
PIN/password pairing, pair verification, FairPlay SAP, receiver capability
negotiation, clocking, encrypted H.264 packetization, and an in-process test
receiver. The protocol code is based on a pinned revision of
[`omarroth/doubletake`](https://github.com/omarroth/doubletake); see
[NOTICE.md](NOTICE.md) and the included LGPL/GPL license texts.

Windows display capture and its command-line picker are being ported on top of
that foundation. End-to-end mirroring has **not** been verified against a real
receiver from this branch yet. See
[the feasibility report](research/mirror-sender-feasibility.md) for the
protocol survey, risks, and validation plan.

## Development

The project requires Go 1.25 or newer.

```console
go test ./...
go build ./cmd/doubletake
```

Do not interpret successful unit tests as real-device validation. Receiver
model, firmware, pairing mode, timing mode, and video acceptance must be
recorded separately during hardware testing.
