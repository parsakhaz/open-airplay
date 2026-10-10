# Mirror sender validation

The Windows display-capture path and a session with the repository's local
modern receiver fixture have passed the checks described below. Physical
Apple TV and HomePod interoperability remains **untested**. The fixture
validates and drains media; it does not render a picture or establish that a
physical receiver will accept or display the stream.

## Environment and observed results

The checks were performed on a Windows host on the `mirror-sender` branch.
FFmpeg reported version `9.0.2-full_build-www.gyan.dev`; Go reported
`go1.27.0 windows/amd64`. The module declares Go `1.25.0` as its minimum
language/toolchain version.

| Check | Observed result | What it establishes |
| --- | --- | --- |
| FFmpeg capabilities | `ddagrab` and `libx264` were available. The build also advertised `h264_mf`, `h264_nvenc`, `h264_qsv`, and `h264_amf`. | The installed build has the Windows Desktop Duplication source and a software H.264 encoder. An advertised hardware encoder is not proof that it can run on this host. |
| Display enumeration | Index `0`: `\\.\DISPLAY1`, `1440x900`, primary. | Native D3D11/DXGI enumeration and the CLI listing found this host's single attached output using the same default-adapter order as FFmpeg `ddagrab`. |
| Capture smoke | Desktop Duplication produced H.264 SPS, PPS, and an IDR access unit. | Capture and software H.264 startup produced the parameter sets and initial keyframe expected by the sender. |
| LAN discovery | mDNS returned nine AirPlay advertisements; six advertised screen mirroring and were retained by the CLI, including multiple `AppleTV14,1` receivers. No session was started on a shared display. | Real receivers are visible to discovery and audio-only services are filtered. This does not establish pairing, setup, decoding, or rendering. Device names and addresses are intentionally omitted. |
| Local modern receiver session | Transient pair-setup, encrypted pair-verify, FairPlay M1-M4, three `SETUP` requests, and `RECORD` completed. The PTP session path, event channel, and feedback ran. | The Windows producer reached the repository's modern protocol fixture through the sender's session flow. |
| Local video transport | `1,130` video packets and `16,557,744` video bytes received; `20` feedback requests; clean `TEARDOWN`. | A bounded local run transported video and shut down cleanly. These are totals from one run, not throughput or stability measurements. |

The session used `-no-audio`; its three-`SETUP` flow therefore does not
establish audio interoperability. Actual audio capture, encoding, transport,
playback, and audio/video synchronization were **not exercised**.

## Reproduce the checks

Run from the repository root. Windows capture requires an interactive desktop
and an FFmpeg build with `ddagrab` and `libx264` on `PATH`. These commands use
the software H.264 baseline explicitly; they do not test a GPU encoder.

Check the tool versions and advertised FFmpeg capabilities:

```powershell
go version
ffmpeg -version
ffmpeg -hide_banner -h filter=ddagrab
ffmpeg -hide_banner -encoders | Select-String 'libx264|h264_mf|h264_nvenc|h264_qsv|h264_amf'
```

List the currently active monitors, then capture the selected display without
contacting a receiver:

```powershell
go run ./cmd/open-airplay -list-displays
go run ./cmd/open-airplay -display 0 -capture-smoke -hwaccel none -no-audio
```

Select an index from the current listing if the display topology differs.
The smoke command succeeds only after startup validates SPS/PPS/IDR; it does
not test receiver decoding or sustained capture. Its validation canvas is
`1280x720`, independent of the desktop's native resolution.

For a local session, start the fixture in one terminal and wait for its
listening message:

```powershell
go run ./cmd/doubletake-test-receiver -listen 127.0.0.1:7000 -profile modern -auth none -debug -stats-interval 1s
```

In a second terminal, create a fresh temporary credentials location and
start the sender:

```powershell
$validationDirectory = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName()))
$validationCredentials = Join-Path $validationDirectory.FullName 'credentials.json'
go run ./cmd/open-airplay -target 127.0.0.1 -port 7000 -display 0 -cred-backend file -creds $validationCredentials -hwaccel none -video-codec h264 -no-audio -debug
```

The fresh credentials path avoids reusing saved pairing records and permits
the transient pairing path to run. Observe the sender's pairing, FairPlay,
setup, recording, and capture logs, and the fixture's increasing video and
feedback counters. Press Ctrl+C once in the sender and allow shutdown to
finish before stopping the fixture with Ctrl+C. Check for `TEARDOWN` and the
fixture's final statistics. Packet and byte totals depend on desktop content
and run length; a rerun is not expected to reproduce the exact totals above.

This direct-target test bypasses mDNS discovery. Discovery was exercised
separately with `go run ./cmd/open-airplay -list-receivers`; it found real
advertisements, but no physical receiver was paired or asked to render.

## Automated checks and CI scope

The workflow in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs
these commands on Windows and Linux, using the Go version in `go.mod`:

```console
go test ./...
go vet ./...
go build ./cmd/...
```

These three commands passed locally on the Windows host with Go 1.27.0.
The Linux job and the hosted GitHub Actions workflow have not been executed
as part of this recorded validation.

CI covers the default Go build, unit tests, and local protocol fixtures. It
does not install capture dependencies, open a Windows desktop capture
session, or attach a physical receiver. Tests that require unavailable
FFmpeg, GStreamer, D-Bus, Xvfb, or receiver hardware may skip. The optional
`fdk_aac` build is outside this workflow. A green CI run must not be treated
as evidence of physical AirPlay compatibility.

## Remaining validation

- Physical Apple TV and HomePod devices: untested, including model/firmware
  differences, PIN/password flows, saved-credential reconnection, and visible
  video acceptance.
- Actual audio capture, streaming, playback, and audio/video synchronization:
  not exercised.
- Long-duration streaming, recovery after network interruption, and resource
  growth: unverified.
- Capture-to-display latency, delivered frame rate, CPU/GPU load, dropped
  frames, and bitrate behavior: unmeasured.
- Multiple outputs on the default adapter, docking/topology changes, mixed DPI,
  rotated displays, and HDR: unverified. Outputs attached only to another GPU
  adapter are intentionally not listed because default `ddagrab` cannot address
  them.
- Hardware H.264 encoder execution and output acceptance: unverified;
  capability listings alone do not validate these paths.

Real-device validation must record the receiver model and firmware, pairing
mode, negotiated canvas and timing/encryption mode, visible SPS/PPS/IDR
acceptance, stream duration, latency, and teardown. Until then the status is
Windows capture and local protocol integration validated, physical hardware
compatibility unverified.
