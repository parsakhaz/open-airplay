//! Test-only fake receiver engine. Stands in for the real engine so QA can drive
//! every receive state deterministically without an iPhone.
//!
//! Build: `cargo build --manifest-path src-tauri/Cargo.toml --example fake_engine`
//! Use:   `set OAP_ENGINE=<repo>\target\debug\examples\fake_engine.exe` and start the app.
//!
//! It ignores UxPlay-style args (`-n <name> -nh -d [-pin] [-h265]`) except `-pin`,
//! prints a few noise lines, then the advertised line after ~300 ms
//! (skipped when `OAP_FAKE_NO_ADVERTISE=1`).
//!
//! Commands are read from stdin and from a control file
//! (`OAP_FAKE_CONTROL`, default `%TEMP%\oap-fake-engine.ctl`; only lines appended
//! after start are processed, polled every 100 ms), one per line:
//!
//! - `advertise`                 advertised line
//! - `connect [name] [model]`    `Open connections: 1` + connection request (+ pin line if `-pin`)
//! - `video [WxH]`               `open-airplay: video WxH` (default 886x1920)
//! - `video-stop`                `open-airplay: video stopped`
//! - `pin <code>`                `open-airplay: pin = <code>`
//! - `end`                       video stopped (if video) + `Open connections: 0`
//! - `lost`                      client-lost error + `Open connections: 0`
//! - `error <message>`           `*** ERROR: <message>`
//! - `storm <lines_per_second>`  noise lines at that rate until `storm 0`
//! - `exit <code>`               exit the process with that code
//! - `say <text>`                print text verbatim
//! - `disconnect`                end the session (if any) then `open-airplay: disconnected`
//! - `audio [name] [model]`      a phone picks this PC as a speaker: `connect` + `open-airplay: audio start`
//! - `audio-stop`                `open-airplay: audio stopped`
//! - `track title|artist|album`  `open-airplay: track …` (fields may be empty)
//! - `art <image file>`          copies the image to the `-ca` file, then `open-airplay: art <bytes>`
//! - `volume <0-100>`            `open-airplay: volume <n>`
//! - `progress`                  two progress lines ending in a carriage return, as UxPlay prints them
//! - `dacp`                      starts a fake phone remote (mDNS `_dacp._tcp` + HTTP) and prints
//!   `open-airplay: dacp <id> <token>`; each command it gets is printed
//!   as `fake-engine: remote got <path> (Active-Remote <token>)`
//!
//! Example: `echo connect>> %TEMP%\oap-fake-engine.ctl`

use std::fs::File;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const ADVERTISED: &str =
    "register_dnssd: advertised AirPlay service with \"Features\" code = 0x5A7FFEE6";
const DEFAULT_PIN: &str = "4821";
const DEVICE_ID: &str = "AA:BB:CC:DD:EE:01";

#[derive(Default)]
struct Session {
    connected: bool,
    video: bool,
}

struct Ctx {
    pin_mode: bool,
    /// The `-ca <file>` path the shell passed.
    art_path: Option<PathBuf>,
    /// The fake phone remote, once started (kept alive for the process).
    remote: Mutex<Option<mdns_sd::ServiceDaemon>>,
    session: Mutex<Session>,
    storm_rate: AtomicU64,
}

/// Write one line and flush; errors (closed pipe) are ignored rather than panicking.
fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn lock(ctx: &Ctx) -> std::sync::MutexGuard<'_, Session> {
    ctx.session.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stop video (if running) and drop the connection count to 0.
fn close_session(s: &mut Session) {
    if s.video {
        emit("open-airplay: video stopped");
    }
    s.video = false;
    s.connected = false;
    emit("Open connections: 0");
}

fn handle(ctx: &Ctx, raw: &str) {
    let line = raw.trim();
    if line.is_empty() {
        return;
    }
    let (cmd, rest) = match line.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };
    match cmd {
        "advertise" => emit(ADVERTISED),
        "connect" => {
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("Test iPhone");
            let model = parts.next().unwrap_or("iPhone16,1");
            let mut s = lock(ctx);
            s.connected = true;
            emit("Open connections: 1");
            emit(&format!(
                "connection request from {name} ({model}) with deviceID = {DEVICE_ID}"
            ));
            if ctx.pin_mode {
                emit(&format!("open-airplay: pin = {DEFAULT_PIN}"));
            }
        }
        "video" => {
            let size = if rest.is_empty() { "886x1920" } else { rest };
            let valid = size
                .split_once('x')
                .map(|(w, h)| w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok())
                .unwrap_or(false);
            if !valid {
                emit(&format!("fake-engine: bad video size {size:?}"));
                return;
            }
            lock(ctx).video = true;
            emit(&format!("open-airplay: video {size}"));
        }
        "video-stop" => {
            lock(ctx).video = false;
            emit("open-airplay: video stopped");
        }
        "pin" => {
            let code = if rest.is_empty() { DEFAULT_PIN } else { rest };
            emit(&format!("open-airplay: pin = {code}"));
        }
        "end" => close_session(&mut lock(ctx)),
        "lost" => {
            let mut s = lock(ctx);
            emit("*** ERROR lost connection with client (network problem?)");
            s.video = false;
            s.connected = false;
            emit("Open connections: 0");
        }
        "error" => {
            let msg = if rest.is_empty() { "fake engine error" } else { rest };
            emit(&format!("*** ERROR: {msg}"));
        }
        "storm" => match rest.parse::<u64>() {
            Ok(rate) => {
                ctx.storm_rate.store(rate, Ordering::SeqCst);
                emit(&format!("fake-engine: storm rate = {rate}"));
            }
            Err(_) => emit(&format!("fake-engine: bad storm rate {rest:?}")),
        },
        "exit" => {
            let code = rest.parse::<i32>().unwrap_or(1);
            emit(&format!("fake-engine: exiting with code {code}"));
            std::process::exit(code);
        }
        "say" => emit(rest),
        "disconnect" => {
            let mut s = lock(ctx);
            if s.connected || s.video {
                close_session(&mut s);
            }
            emit("open-airplay: disconnected");
        }
        "audio" => {
            handle(ctx, &format!("connect {rest}"));
            emit("open-airplay: audio start");
        }
        "audio-stop" => emit("open-airplay: audio stopped"),
        "track" => {
            let mut f = rest.splitn(3, '|');
            let (t, a, al) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
            emit(&format!("open-airplay: track {t}\t{a}\t{al}"));
        }
        "art" => match (&ctx.art_path, std::fs::read(rest)) {
            (Some(dest), Ok(bytes)) if std::fs::write(dest, &bytes).is_ok() => emit(&format!("open-airplay: art {}", bytes.len())),
            _ => emit(&format!("fake-engine: can't copy art {rest:?} (is -ca set?)")),
        },
        "volume" => emit(&format!("open-airplay: volume {rest}")),
        "progress" => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(b"audio progress (min:sec):   0:41; remaining:   3:12; track length 3:53\r");
            let _ = out.write_all(b"audio progress (min:sec):   0:42; remaining:   3:11; track length 3:53\r");
            let _ = out.flush();
        }
        "dacp" => start_remote(ctx),
        _ => emit(&format!("fake-engine: unknown command {line:?}")),
    }
}

/// A stand-in for the phone's remote-control service: advertised like a phone advertises it, and it
/// prints each command it receives.
fn start_remote(ctx: &Ctx) {
    const ID: &str = "FA4E00000000D0AC";
    const TOKEN: &str = "1986535575";
    let mut remote = ctx.remote.lock().unwrap_or_else(|e| e.into_inner());
    if remote.is_none() {
        let started = (|| -> Result<mdns_sd::ServiceDaemon, String> {
            let listener = std::net::TcpListener::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    serve_remote(conn);
                }
            });
            let ip = local_ip().ok_or("no network address")?;
            let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
            let info = mdns_sd::ServiceInfo::new("_dacp._tcp.local.", &format!("iTunes_Ctrl_{ID}"), "oap-fake-phone.local.", ip, port, None::<std::collections::HashMap<String, String>>)
                .map_err(|e| e.to_string())?;
            mdns.register(info).map_err(|e| e.to_string())?;
            emit(&format!("fake-engine: remote listening on {ip}:{port}"));
            Ok(mdns)
        })();
        match started {
            Ok(mdns) => *remote = Some(mdns),
            Err(e) => return emit(&format!("fake-engine: can't start the remote: {e}")),
        }
    }
    emit(&format!("open-airplay: dacp {ID} {TOKEN}"));
}

fn serve_remote(mut conn: std::net::TcpStream) {
    let mut buf = [0u8; 2048];
    let n = conn.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.split_whitespace().nth(1).unwrap_or("?").to_string();
    let token = req.lines().find_map(|l| l.strip_prefix("Active-Remote: ")).unwrap_or("none").to_string();
    emit(&format!("fake-engine: remote got {path} (Active-Remote {token})"));
    let _ = conn.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
}

/// The address phones would reach this PC on (the interface with the default route).
fn local_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

fn control_path() -> PathBuf {
    match std::env::var_os("OAP_FAKE_CONTROL") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => std::env::temp_dir().join("oap-fake-engine.ctl"),
    }
}

/// Poll the control file and run complete lines appended since startup.
fn watch_control(ctx: Arc<Ctx>, path: PathBuf) {
    let mut offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    loop {
        thread::sleep(Duration::from_millis(100));
        let len = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(_) => {
                offset = 0; // deleted: a re-created file is read from the start
                continue;
            }
        };
        if len < offset {
            offset = 0; // truncated
        }
        if len == offset {
            continue;
        }
        let mut buf = Vec::new();
        let read = File::open(&path).and_then(|mut f| {
            f.seek(SeekFrom::Start(offset))?;
            f.take(len - offset).read_to_end(&mut buf)
        });
        if read.is_err() {
            continue;
        }
        // Only consume complete lines; a partial trailing line waits for its newline.
        let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
            continue;
        };
        offset += last_nl as u64 + 1;
        for line in String::from_utf8_lossy(&buf[..=last_nl]).lines() {
            handle(&ctx, line);
        }
    }
}

fn storm(ctx: Arc<Ctx>) {
    let mut n: u64 = 0;
    let mut current = 0u64;
    let mut start = Instant::now();
    let mut sent: u64 = 0;
    loop {
        let rate = ctx.storm_rate.load(Ordering::SeqCst);
        if rate != current {
            current = rate;
            start = Instant::now();
            sent = 0;
        }
        if rate == 0 {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        let due = (start.elapsed().as_secs_f64() * rate as f64) as u64;
        while sent < due {
            n += 1;
            sent += 1;
            emit(&format!(
                "raop_rtp_mirror: storm line {n} seq={} ts={} nal_units=3 bytes=4096",
                n * 7,
                n * 3000
            ));
            if ctx.storm_rate.load(Ordering::SeqCst) != current {
                break;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ctx = Arc::new(Ctx {
        pin_mode: args.iter().any(|a| a == "-pin"),
        art_path: args.iter().position(|a| a == "-ca").and_then(|i| args.get(i + 1)).map(PathBuf::from),
        remote: Mutex::new(None),
        session: Mutex::new(Session::default()),
        storm_rate: AtomicU64::new(0),
    });

    emit("UxPlay 1.72 (fake engine): an open-source AirPlay mirroring server");
    emit(&format!("fake-engine: args = {}", args.join(" ")));
    let path = control_path();
    emit(&format!("fake-engine: control file = {}", path.display()));

    {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => handle(&ctx, &l),
                    Err(_) => break,
                }
            }
            // EOF (or NUL stdin): keep running; other threads drive the engine.
        });
    }
    {
        let ctx = ctx.clone();
        thread::spawn(move || watch_control(ctx, path));
    }
    {
        let ctx = ctx.clone();
        thread::spawn(move || storm(ctx));
    }

    thread::sleep(Duration::from_millis(300));
    if std::env::var("OAP_FAKE_NO_ADVERTISE").as_deref() != Ok("1") {
        emit(ADVERTISED);
    }
    loop {
        thread::park();
    }
}
