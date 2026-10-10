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
- `design/tokens.css`: design tokens shared with open-airdrop-windows (copied into `ui/` at build time).

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

## Tests

```powershell
npm test   # Rust unit tests (engine seam, supervisor with a fake engine, sender protocol) + sender sidecar tests
```

## License

GPL-3.0 (see `LICENSE`), because the receiver uses UxPlay (GPL-3.0). Third-party components are listed in `NOTICE`.
