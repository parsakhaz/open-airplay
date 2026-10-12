# Developing open-airplay

How the pieces fit, how to run each one, how to build the installer, and how to run the tests. The project overview is in the [README](../README.md).

## How it fits together

```
iPhone / Mac ──AirPlay──▶ uxplay.exe (UxPlay 1.74 fork, built-in mDNS, no Bonjour)
                              │ stdout lines (patched: unbuffered + machine-readable PIN)
                              ▼
                     Tauri 2 shell (Rust core + web UI, tray)
                              │ JSON lines over stdin/stdout
                              ▼
                     sender/oap_sender.py (pyatv) ──AirPlay/RAOP──▶ Apple TV / HomePod
```

- `engine/` builds the pinned UxPlay fork (`engine/UXPLAY_PIN`, patches in `engine/patches/`).
- `src-tauri/src/engine.rs`: settings → UxPlay flags, UxPlay output → events (the engine seam).
- `src-tauri/src/supervisor.rs`: runs the engine without a console window, restarts it if it exits.
- `src-tauri/src/sender.rs` + `sender/oap_sender.py`: the pyatv sender sidecar.
- `design/`: the design system shared with open-airdrop-windows (see below).

## Run it (Windows, development)

Prerequisites: [MSYS2](https://www.msys2.org) at `C:\msys64`, Rust (rustup, MSVC toolchain), Node 20+, Python 3.10+.

```powershell
# 1. Build the receiver engine (installs MSYS2 packages: gcc, cmake, GStreamer, libplist)
C:\msys64\usr\bin\bash.exe -lc "/c/path/to/open-airplay/engine/build-engine.sh"

# 2. Sender sidecar
python -m venv sender\.venv; sender\.venv\Scripts\python -m pip install -r sender\requirements.txt

# 3. App
npm install
npm run dev
```

A manual launch opens the tray panel and restores the receiving switch as you left it (on the first time). On the iPhone: Control Center → Screen Mirroring → pick the PC's name.
The first launch shows a Windows Firewall prompt for `uxplay.exe`; allow it, or iPhones cannot connect.
On a network set to **Public**, Windows allows the app only if you tick Public in that prompt.

Start at login is on by default. Sign-in launches pass `--autostart`: the app stays in the tray, opens no window and leaves receiving off until you flip the switch. Dev builds never touch the Run entry (it has the same name as the installed app's) unless `OAP_DEV_AUTOSTART=1` is set; to try a boot launch in development, run `npm run tauri dev -- -- --autostart`.

Engine output is logged to `%LOCALAPPDATA%\org.openairplay.app\logs\engine.log`. Each app start moves the previous run's log to `engine.1.log`, and the log also rotates when it reaches 5 MB. Three older files are kept, from `engine.1.log` (newest) to `engine.3.log`. The app runs the engine without UxPlay's `-d`, so the log has no debug output. For that, turn receiving off and run the engine yourself, for example `engine\dist\uxplay.exe -n test-receiver -nh -d` (with `C:\msys64\ucrt64\bin` on `PATH`).

Overrides: `OAP_ENGINE` (path to uxplay.exe), `OAP_MSYS2_ROOT` (default `C:\msys64`), `OAP_PYTHON` (sender interpreter).

## Design system

`design/` is the shared look for open-airplay and open-airdrop-windows. It has a neutral, macOS-like style with one blue accent, light and dark themes, and reduced-motion support.

- `design/tokens.css` holds the variables only, all prefixed `--oai-`: colours, type, spacing, radii, shadows, blur materials and motion timings. Light and dark follow `prefers-color-scheme`. You can force a theme with `<html data-theme="light|dark">`.
- `design/components.css` holds the building blocks, as classes prefixed `oai-`:
  - window chrome: `.oai-window`, `.oai-titlebar`, `.oai-window-controls`
  - layout: `.oai-sidebar`, `.oai-toolbar`, `.oai-stage`
  - the tray popover: `.oai-popover`, `.oai-list-row`
  - controls: `.oai-btn`, `.oai-switch`, `.oai-segmented`, `.oai-input`, grouped settings lists (`.oai-group`, `.oai-row`)
  - feedback: `.oai-card`, `.oai-spinner`, `.oai-hero` (empty states), `.oai-banner`
- `design/icon.svg` is the app icon. Regenerate the bundle icons with `npx tauri icon design/icon.svg -o src-tauri/icons`, then delete the mobile and Store variants.

To use it in another app, such as open-airdrop-windows:
1. Copy `design/` in, or reference it as a git subtree.
2. Copy both stylesheets into the web root at build time. This repo does that with `npm run sync-tokens`, which `npm run dev` and `beforeBuildCommand` already run. Keep the copies gitignored.
3. Load them before your own styles: `tokens.css`, then `components.css`, then `app.css`.
4. Use only `--oai-*` variables in app styles, so both apps keep the same look and both themes keep working.

The app window has two modes built from these pieces. The **compact** mode is the primary surface: a frosted popover above the tray with status, a receiving switch, Send to devices and quick settings. The **full** mode opens on demand with Expand, or automatically when mirroring starts if you turn that on in Settings. It has a device sidebar and a large stage. F11 toggles full screen, and Esc returns to compact.

## Build the installer (Windows)

```powershell
C:\msys64\usr\bin\bash.exe -lc "/c/path/to/open-airplay/engine/build-engine.sh"   # builds the engine and stages engine/runtime
npm run build                                                                       # -> src-tauri/target/release/bundle/nsis/*-setup.exe
```

- `engine/stage-runtime.sh` (run by `build-engine.sh`) copies `uxplay.exe`, the 17 GStreamer plugins UxPlay's pipelines use, and every DLL they load into `engine/runtime/` (gitignored): `bin/` holds the exe and DLLs, `lib/gstreamer-1.0/` holds the plugins. GStreamer finds the plugins relative to its own DLL, so the installed app needs no MSYS2, no PATH changes and no `GST_PLUGIN_*` variables.
- `src-tauri/tauri.windows.conf.json` (merged on Windows only, so macOS and Linux builds keep their own targets) bundles that folder as `engine/` next to the app and builds a per-machine NSIS installer. The installer asks for administrator rights, because it adds a firewall rule.
- `src-tauri/windows/hooks.nsh`: on install, the installer adds one inbound firewall rule, "open-airplay receiver", scoped to `<install dir>\engine\bin\uxplay.exe`. On uninstall, it stops the app and removes the rule.
- Size: the runtime is about 174 MB on disk and the installer is about 49 MB. Most of that is MSYS2's FFmpeg, which links every codec library (x265, aom, svt-av1, …) even though UxPlay only decodes H.264/H.265, AAC and ALAC. A minimal FFmpeg build would shrink it a lot; that is a follow-up.
- The installer is unsigned for now (code signing is an ask-first decision), so Windows SmartScreen warns on first run.

## FairPlay desktop mirror sender (Go)

The sender that mirrors this PC's desktop to an Apple TV lives in `cmd/open-airplay` (CLI), `internal/airplay` (protocol: discovery, pairing, FairPlay SAP, encrypted H.264 mirroring, capture) and `internal/daemon`. Its protocol core comes from [doubletake](https://github.com/omarroth/doubletake); provenance is in `NOTICE`. It needs Go 1.25+ and an FFmpeg build with `ddagrab` on `PATH`; usage is in the README's quick start.

- `go run ./cmd/open-airplay -list-displays` and `-display 0 -capture-smoke -hwaccel none -no-audio` check capture without contacting a receiver.
- `go run ./cmd/doubletake-test-receiver -listen 127.0.0.1:7000 -profile modern -auth none` is a local receiver fixture for protocol tests.
- Validation evidence and limits: `research/mirror-sender-validation.md`; protocol survey: `research/mirror-sender-feasibility.md`.
- The Go CLI builds an executable also named `open-airplay.exe`; it is separate from the Tauri app (not yet folded into its UI or installer).

## Tests

```powershell
npm test        # Rust unit tests (engine seam, supervisor, sender bridge, prefs, doctor) + pyatv sidecar tests
go test ./...   # FairPlay mirror sender
go vet ./...
```
The engine's C tests (window sizing, maximized/full-screen reshow) run inside `engine/build-engine.sh`. On Windows, Tauri's build script needs `engine/runtime/` to exist (it is the installer's engine resource); `build-engine.sh` creates it, or `mkdir engine/runtime` is enough for running the Rust tests. CI runs all of these on Windows, plus the Rust check and tests and the Go suite on Linux.
