# Mirror sender validation

The Windows display-capture path and a session with the repository's local
modern receiver fixture have passed the checks described below. On October 10,
2026, an owner-run session also verified end-to-end video mirroring on a
physical Apple TV 4K (`AppleTV14,1`): pairing and FairPlay setup completed, the
mirror session started, and the real Windows desktop was visible on the TV.
The owner described latency as near-instant. That is a qualitative observation,
not an instrumented latency measurement. The automatic profile for the
verified run was `1280x720` at `30 fps` and `1843 kbps`.

This is the project's first confirmed real-device FairPlay mirror. It
establishes interoperability with one receiver model and configuration, not
every Apple TV firmware, Apple receiver, or third-party implementation. The
local fixture still provides separate repeatable protocol and transport
coverage; it validates and drains media but does not render a picture.

## Environment and observed results

The checks were performed on a Windows host on the `mirror-sender` branch.
FFmpeg reported version `9.0.2-full_build-www.gyan.dev`; Go reported
`go1.27.0 windows/amd64`. The module declares Go `1.25.0` as its minimum
language/toolchain version.

| Check | Observed result | What it establishes |
| --- | --- | --- |
| FFmpeg capabilities | `ddagrab` and `libx264` were available. The build also advertised `h264_mf`, `h264_nvenc`, `h264_qsv`, and `h264_amf`. | The installed build has the Windows Desktop Duplication source and a software H.264 encoder. An advertised hardware encoder is not proof that it can run on this host. |
| Display enumeration | Index `0`: `\\.\DISPLAY1`, `1440x900`, primary. | Native D3D11/DXGI enumeration and the CLI listing found this host's single attached output using the same default-adapter order as FFmpeg `ddagrab`. |
| Capture smoke | Desktop Duplication produced H.264 SPS, PPS, and an IDR access unit. After the fidelity update, the receiver-free smoke was rerun successfully at the selected display's native `1440x900`, `60 fps`, and auto `5184 kbps`. | Capture and software H.264 startup produced the parameter sets and initial keyframe expected by the sender at the new local profile. This does not establish physical-receiver decoding or latency at that profile. |
| LAN discovery | mDNS returned nine AirPlay advertisements; six advertised screen mirroring and were retained by the CLI, including multiple `AppleTV14,1` receivers. Discovery itself did not open a receiver session. A later mirror session targeted only the owner-designated receiver; no pairing, control, or media session was opened with another receiver. | Real receivers are visible to discovery and audio-only services are filtered. Discovery alone does not establish pairing, setup, decoding, or rendering. Device names and addresses are intentionally omitted. |
| Local modern receiver session | Transient pair-setup, encrypted pair-verify, FairPlay M1-M4, three `SETUP` requests, and `RECORD` completed. The PTP session path, event channel, and feedback ran. | The Windows producer reached the repository's modern protocol fixture through the sender's session flow. |
| Local video transport | `1,130` video packets and `16,557,744` video bytes received; `20` feedback requests; clean `TEARDOWN`. | A bounded local run transported video and shut down cleanly. These are totals from one run, not throughput or stability measurements. |
| Physical Apple TV 4K | The owner-designated `AppleTV14,1` completed pairing and FairPlay setup, started the mirror session, and displayed the real Windows desktop at the automatic `1280x720`, `30 fps`, `1843 kbps` profile. The owner reported near-instant perceived latency. | Visible, real-device FairPlay video mirroring is confirmed on one physical Apple TV model. Latency was observed by the user rather than measured; audio, soak, reconnect, and broader hardware coverage remain open. |

Windows currently enforces video-only operation because a WASAPI capture path
has not been implemented. Neither the local fixture session nor the physical
Apple TV result establishes audio interoperability. Actual audio capture,
encoding, transport, playback, and audio/video synchronization were not
exercised.

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
not test receiver decoding or sustained capture. The recorded smoke evidence
above was rerun after the fidelity update. The current command uses the
selected display's native dimensions and `60 fps` when `-fps` is omitted; on
this host it passed at `1440x900`/`60 fps` with auto `5184 kbps`. That revised
profile still needs a physical-receiver regression.

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

This direct-target fixture test bypasses mDNS discovery. Discovery was
exercised separately with `go run ./cmd/open-airplay -list-receivers`; it found
real advertisements. The later physical validation selected only the
owner-designated Apple TV and produced visible video. Device names and network
addresses are omitted intentionally.

## Automated checks and CI scope

The workflow in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs
these commands on Windows and Linux, using the Go version in `go.mod`:

```console
go test ./...
go vet ./...
go build ./cmd/...
```

These three commands passed locally on the Windows host with Go 1.27.0. Hosted
Windows and Linux jobs are configured on PR #2; their current result should be
checked on the pushed commit rather than inferred from an earlier green run.

CI covers the default Go build, unit tests, and local protocol fixtures. It
does not install capture dependencies, open a Windows desktop capture
session, or attach a physical receiver. Tests that require unavailable
FFmpeg, GStreamer, D-Bus, Xvfb, or receiver hardware may skip. The optional
`fdk_aac` build is outside this workflow. A green CI run alone must not be
treated as evidence of physical AirPlay compatibility; the separate owner-run
Apple TV result above provides that evidence for one model and configuration.

## Remaining validation

- Apple TV 4K (`AppleTV14,1`) pairing, FairPlay setup, and visible video are
  confirmed for one owner-run session. Its firmware version, saved-credential
  reconnect, wrong-PIN and cancellation behavior, other Apple TV models, Macs,
  HomePods, and third-party receivers remain unverified.
- Actual audio capture, streaming, playback, and audio/video synchronization:
  not exercised; Windows WASAPI capture is not implemented.
- Long-duration streaming, 30-60 minute soak behavior, saved-credential
  reconnect, recovery after network interruption, resource growth, and error
  UX remain unverified.
- The owner reported near-instant perceived latency on the confirmed run, but
  instrumented capture-to-display latency, first-frame time, delivered frame
  rate, CPU/GPU load, dropped frames, and bitrate behavior remain unmeasured.
- The packaged/shipped Windows path, installer and FFmpeg distribution, and
  by-name `-receiver` selection from that shipped build remain unverified. The
  real-device result used the development workflow.
- The confirmed hardware run used the automatic `1280x720` at `30 fps` / `1843
  kbps` profile. Native-resolution and 60 fps defaults still require
  real-device regression and sustained-load validation.
- Multiple outputs on the default adapter, docking/topology changes, mixed DPI,
  rotated displays, and HDR: unverified. Outputs attached only to another GPU
  adapter are intentionally not listed because default `ddagrab` cannot address
  them.
- Hardware H.264 encoder execution and output acceptance: unverified;
  capability listings alone do not validate these paths.

Current status: Windows capture, local protocol integration, and visible
FairPlay video mirroring on one physical Apple TV 4K are validated. Future
hardware records should add firmware, exact pairing mode, negotiated timing,
stream duration, measured latency, and teardown evidence. Audio and broader
receiver, firmware, display, GPU, and packaged-build coverage remain
unverified.
