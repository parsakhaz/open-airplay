# open-airplay

Open-source AirPlay for Windows with a UX that doesn't suck: receive (iPhone/Mac → PC) and send (PC → Apple TV, HomePod, AirPlay speakers).

Status: early. The receiver and sender work end to end in development; installer, accept prompt, mirror-window control and the network doctor are next (see the plan).

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

The app starts receiving immediately. On the iPhone: Control Center → Screen Mirroring → pick the PC's name.
The first launch shows a Windows Firewall prompt for `uxplay.exe`; allow it, or iPhones cannot connect.
On a network set to **Public**, Windows allows the app only if you tick Public in that prompt.

Engine output is logged to `%LOCALAPPDATA%\org.openairplay.app\logs\engine.log`.

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

## Tests

```powershell
npm test   # Rust unit tests (engine seam, supervisor with a fake engine, sender protocol) + sender sidecar tests
```

## License

GPL-3.0 (see `LICENSE`), because the receiver uses UxPlay (GPL-3.0). Third-party components are listed in `NOTICE`.
