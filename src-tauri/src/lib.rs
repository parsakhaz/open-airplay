pub mod doctor;
pub mod engine;
pub mod launch;
pub mod netwatch;
pub mod prefs;
pub mod process_guard;
pub mod sender;
pub mod supervisor;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, WebviewWindow};

use engine::{EngineEvent, ReceiverSettings};
use prefs::AppPrefs;
use sender::SenderSidecar;
use supervisor::{EngineCommand, Supervisor, SupervisorEvent};

/// What the receiver is doing, as the UI shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Stopped,
    Starting,
    Ready,
    Connecting,
    Mirroring,
    Pin,
    Error,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReceiverStatus {
    pub phase: Phase,
    /// The user wants the receiver on (Start/Apply pressed, or auto-start); false after Stop.
    pub wanted: bool,
    pub running: bool,
    pub engine_path: Option<String>,
    pub settings: Option<ReceiverSettings>,
    pub advertised: bool,
    pub connections: u32,
    pub last_device: Option<String>,
    /// Size of the mirrored video once frames arrive (reported by engine patch 0002).
    pub video: Option<(u32, u32)>,
    /// Unix time in ms when the current mirroring session's video started.
    pub mirroring_since_ms: Option<u64>,
    pub pin: Option<String>,
    pub last_error: Option<String>,
}

pub fn phase(s: &ReceiverStatus) -> Phase {
    if !s.running {
        return match (s.wanted, s.last_error.is_some()) {
            (false, _) => Phase::Stopped,
            (true, true) => Phase::Error,
            (true, false) => Phase::Starting,
        };
    }
    if s.pin.is_some() {
        Phase::Pin
    } else if s.video.is_some() {
        Phase::Mirroring
    } else if s.last_device.is_some() && s.connections > 0 {
        Phase::Connecting
    } else if s.advertised {
        Phase::Ready
    } else {
        Phase::Starting
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn end_session(status: &mut ReceiverStatus) {
    status.connections = 0;
    status.last_device = None;
    status.video = None;
    status.mirroring_since_ms = None;
    status.pin = None;
}

pub fn apply_event(status: &mut ReceiverStatus, event: &SupervisorEvent) {
    match event {
        SupervisorEvent::Started { .. } => {
            status.running = true;
            status.last_error = None;
        }
        SupervisorEvent::Exited { code } => {
            status.running = false;
            status.advertised = false;
            end_session(status);
            if status.wanted {
                status.last_error = Some(format!("The receiver stopped unexpectedly (exit code {code:?}). Restarting…"));
            }
        }
        SupervisorEvent::SpawnFailed { message } => {
            status.running = false;
            status.last_error = Some(format!("Could not start the receiver: {message}"));
        }
        SupervisorEvent::Engine { event } => match event {
            EngineEvent::Advertised => status.advertised = true,
            EngineEvent::ConnectionRequest { name, model, .. } => status.last_device = Some(format!("{name} ({model})")),
            EngineEvent::OpenConnections { count } => {
                if *count == 0 {
                    end_session(status);
                } else {
                    status.connections = *count;
                }
            }
            EngineEvent::Video { width, height } => {
                status.video = Some((*width, *height));
                status.pin = None;
                status.mirroring_since_ms.get_or_insert_with(now_ms);
            }
            EngineEvent::Pin { code } => status.pin = Some(code.clone()),
            EngineEvent::ClientLost => status.last_error = Some("Lost the connection to the sender.".into()),
            EngineEvent::Error { message } => status.last_error = Some(message.clone()),
        },
        SupervisorEvent::Log { .. } => {}
    }
    status.phase = phase(status);
}

/// The processes the app owns. `shutdown` stops and reaps all of them; it runs on every exit path.
#[derive(Default)]
pub struct Sidecars {
    supervisor: Mutex<Option<Supervisor>>,
    /// (generation, sidecar): the generation lets a late exit notice skip a newer sidecar.
    sender: Mutex<Option<(u64, SenderSidecar)>>,
    sender_generation: AtomicU64,
}

impl Sidecars {
    pub fn replace_supervisor(&self, sup: Option<Supervisor>) {
        let old = std::mem::replace(&mut *self.supervisor.lock().unwrap(), sup);
        if let Some(old) = old {
            old.stop();
        }
    }

    /// The running engine's process id, if any.
    pub fn engine_pid(&self) -> Option<u32> {
        self.supervisor.lock().unwrap().as_ref().and_then(Supervisor::pid)
    }

    pub fn shutdown(&self) {
        self.replace_supervisor(None);
        let sender = self.sender.lock().unwrap().take();
        if let Some((_, s)) = sender {
            s.stop();
        }
    }
}

#[derive(Default)]
struct AppState {
    /// Latest network-doctor findings, shown as cards in the UI.
    findings: Mutex<Vec<doctor::Finding>>,
    sidecars: Sidecars,
    status: Arc<Mutex<ReceiverStatus>>,
    prefs: Mutex<AppPrefs>,
    /// Where prefs are saved; set during setup (None in tests or if there's no config folder).
    prefs_path: Mutex<Option<PathBuf>>,
}

impl AppState {
    fn update_prefs(&self, change: impl FnOnce(&mut AppPrefs)) -> AppPrefs {
        let mut prefs = self.prefs.lock().unwrap();
        change(&mut prefs);
        if let Some(path) = self.prefs_path.lock().unwrap().as_ref() {
            if let Err(e) = prefs.save(path) {
                eprintln!("open-airplay: could not save settings to {}: {e}", path.display());
            }
        }
        prefs.clone()
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) { format!("{stem}.exe") } else { stem.to_string() }
}

/// Where the engine comes from decides how its runtime is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EngineSource {
    /// OAP_ENGINE or engine/dist from build-engine.sh: needs the MSYS2 GStreamer runtime.
    Dev,
    /// Installed with the app (engine/bin + engine/lib/gstreamer-1.0): GStreamer finds its plugins by itself.
    Bundled,
}

/// The installed engine wins, then OAP_ENGINE, then the dev build (engine/dist).
fn find_engine(app: &AppHandle) -> Option<(PathBuf, EngineSource)> {
    let mut candidates: Vec<(PathBuf, EngineSource)> = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        candidates.push((res.join("engine").join("bin").join(exe_name("uxplay")), EngineSource::Bundled));
    }
    if let Some(p) = std::env::var_os("OAP_ENGINE") {
        candidates.push((PathBuf::from(p), EngineSource::Dev));
    }
    candidates.push((repo_root().join("engine").join("dist").join(exe_name("uxplay")), EngineSource::Dev));
    candidates.into_iter().find(|(p, _)| p.is_file())
}

/// The app icon, for the engine's mirror window: dev checkout first, then the bundled resource.
fn find_window_icon(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = vec![repo_root().join("src-tauri").join("icons").join("icon.ico")];
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join("icons").join("icon.ico"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn default_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "open-airplay".into())
}

/// Dev runtime: the MSYS2 UCRT64 GStreamer that build-engine.sh installed.
fn engine_runtime() -> (Vec<PathBuf>, Vec<(String, String)>) {
    let msys = PathBuf::from(std::env::var("OAP_MSYS2_ROOT").unwrap_or_else(|_| "C:\\msys64".into())).join("ucrt64");
    if !msys.is_dir() {
        return (vec![], vec![]);
    }
    let plugins = msys.join("lib").join("gstreamer-1.0").to_string_lossy().into_owned();
    (vec![msys.join("bin")], vec![("GST_PLUGIN_SYSTEM_PATH_1_0".into(), plugins)])
}

fn tray_text(status: &ReceiverStatus) -> String {
    let name = status.settings.as_ref().map(|s| s.name.as_str()).unwrap_or("this PC");
    match status.phase {
        Phase::Mirroring => format!("open-airplay: mirroring on '{name}'"),
        Phase::Connecting => "open-airplay: connecting…".into(),
        Phase::Pin => "open-airplay: enter the code on your device".into(),
        Phase::Ready => format!("open-airplay: ready to receive as '{name}'"),
        Phase::Starting => "open-airplay: starting…".into(),
        Phase::Error => "open-airplay: needs attention".into(),
        Phase::Stopped => "open-airplay: receiving is off".into(),
    }
}

/// Which tray icon a phase shows: the plain app icon, a green "receiving" dot, or an orange "problem" dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayLook {
    Normal = 0,
    Receiving = 1,
    Problem = 2,
}

fn tray_look(phase: Phase) -> TrayLook {
    match phase {
        Phase::Connecting | Phase::Mirroring | Phase::Pin => TrayLook::Receiving,
        Phase::Error => TrayLook::Problem,
        Phase::Stopped | Phase::Starting | Phase::Ready => TrayLook::Normal,
    }
}

fn set_tray_look(app: &AppHandle, tray: &tauri::tray::TrayIcon, look: TrayLook) {
    static CURRENT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(TrayLook::Normal as u8);
    if CURRENT.swap(look as u8, Ordering::SeqCst) == look as u8 {
        return;
    }
    let icon = match look {
        TrayLook::Normal => app.default_window_icon().cloned(),
        TrayLook::Receiving => tauri::image::Image::from_bytes(include_bytes!("../icons/tray-receiving.png")).ok(),
        TrayLook::Problem => tauri::image::Image::from_bytes(include_bytes!("../icons/tray-problem.png")).ok(),
    };
    if let Some(icon) = icon {
        let _ = tray.set_icon(Some(icon));
    }
}

fn publish(app: &AppHandle, snapshot: &ReceiverStatus) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tray_text(snapshot)));
        set_tray_look(app, &tray, tray_look(snapshot.phase));
    }
    let _ = app.emit("receiver-status", snapshot);
}

fn start_receiver_inner(app: &AppHandle, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    let state = app.state::<AppState>();
    state.update_prefs(|p| p.receiver = settings.clone());
    let (engine, source) = find_engine(app).ok_or("The receiver engine isn't installed. Reinstall open-airplay (or, in development, run engine/build-engine.sh).")?;
    state.sidecars.replace_supervisor(None);
    let (path_prepend, mut env) = match source {
        EngineSource::Dev => engine_runtime(),
        EngineSource::Bundled => (vec![], vec![]),
    };
    env.extend(settings.engine_env(find_window_icon(app).as_deref()));
    let snapshot = {
        let mut st = state.status.lock().unwrap();
        *st = ReceiverStatus {
            wanted: true,
            engine_path: Some(engine.display().to_string()),
            settings: Some(settings.clone()),
            ..Default::default()
        };
        st.phase = phase(&st);
        st.clone()
    };
    publish(app, &snapshot);
    let cmd = EngineCommand { program: engine, args: settings.engine_args(), path_prepend, env, restart_delay: Duration::from_secs(2) };
    let (status, handle) = (state.status.clone(), app.clone());
    // Every engine line and lifecycle event goes to <app log dir>/engine.log for diagnosis.
    let log = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("engine.log")).ok()
    });
    // Runs on the supervisor's delivery thread, never on a thread that waits for the supervisor.
    let sup = Supervisor::start(cmd, move |event| {
        if let Some(mut f) = log.as_ref() {
            use std::io::Write;
            let line = match &event {
                SupervisorEvent::Log { line } => line.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            let _ = writeln!(f, "{line}");
        }
        let snapshot = {
            let mut st = status.lock().unwrap();
            apply_event(&mut st, &event);
            st.clone()
        };
        let _ = handle.emit("receiver-event", &event);
        publish(&handle, &snapshot);
        if matches!(event, SupervisorEvent::Engine { event: EngineEvent::Advertised }) {
            run_doctor(&handle);
        }
    });
    state.sidecars.replace_supervisor(Some(sup));
    let snapshot = state.status.lock().unwrap().clone();
    Ok(snapshot)
}

fn stop_receiver_inner(app: &AppHandle) -> ReceiverStatus {
    let state = app.state::<AppState>();
    state.sidecars.replace_supervisor(None);
    let snapshot = {
        let mut st = state.status.lock().unwrap();
        st.wanted = false;
        st.running = false;
        st.advertised = false;
        st.last_error = None;
        end_session(&mut st);
        st.phase = phase(&st);
        st.clone()
    };
    publish(app, &snapshot);
    snapshot
}

#[tauri::command]
fn receiver_status(app: AppHandle) -> ReceiverStatus {
    app.state::<AppState>().status.lock().unwrap().clone()
}

#[tauri::command]
fn default_settings(app: AppHandle) -> ReceiverSettings {
    app.state::<AppState>().prefs.lock().unwrap().receiver.clone()
}

/// Brings the engine's mirror window to the front (it belongs to the uxplay process).
#[tauri::command]
fn show_video_window() -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
        let class: Vec<u16> = "OpenAirplayMirrorWindow".encode_utf16().chain(Some(0)).collect();
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
            if hwnd.is_null() {
                return Err("The video window isn't open.".into());
            }
            ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(hwnd);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    Err("Not supported on this platform yet.".into())
}

#[tauri::command]
fn get_prefs(app: AppHandle) -> AppPrefs {
    app.state::<AppState>().prefs.lock().unwrap().clone()
}

/// Saves the app-level prefs (receiver settings are saved by receiver_start) and applies start-at-login.
#[tauri::command]
fn set_prefs(app: AppHandle, open_full_view_on_mirror: bool, start_at_login: bool) -> Result<AppPrefs, String> {
    let prefs = app.state::<AppState>().update_prefs(|p| {
        p.open_full_view_on_mirror = open_full_view_on_mirror;
        p.start_at_login = start_at_login;
    });
    apply_start_at_login(&app, prefs.start_at_login)?;
    Ok(prefs)
}

fn apply_start_at_login(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    // Dev builds share the installed app's Run entry name; registering target\debug would hijack it.
    if cfg!(debug_assertions) && std::env::var_os("OAP_DEV_AUTOSTART").is_none() {
        eprintln!("open-airplay: dev build, leaving start at login alone (set OAP_DEV_AUTOSTART=1 to test it)");
        return Ok(());
    }
    let launcher = app.autolaunch();
    let result = if enabled { launcher.enable() } else { launcher.disable() };
    // Disabling when it was never enabled is fine.
    result.or_else(|e| if enabled { Err(format!("Could not turn on start at login: {e}")) } else { Ok(()) })
}

/// Rewrites a live Run entry so one from an older version gains --autostart. An entry the user turned
/// off in Task Manager reads as not enabled and is left alone, since enabling would turn it back on.
fn refresh_start_at_login(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if app.autolaunch().is_enabled().unwrap_or(false) {
        apply_start_at_login(app, true)
    } else {
        Ok(())
    }
}

/// Async + spawn_blocking: stopping the old engine joins a thread, which must never happen on the UI thread.
#[tauri::command]
async fn receiver_start(app: AppHandle, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    app.state::<AppState>().update_prefs(|p| p.receiving = Some(true));
    tauri::async_runtime::spawn_blocking(move || start_receiver_inner(&app, settings)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn receiver_stop(app: AppHandle) -> Result<ReceiverStatus, String> {
    app.state::<AppState>().update_prefs(|p| p.receiving = Some(false));
    tauri::async_runtime::spawn_blocking(move || stop_receiver_inner(&app)).await.map_err(|e| e.to_string())
}

fn sender_python() -> PathBuf {
    std::env::var_os("OAP_PYTHON").map(PathBuf::from).unwrap_or_else(|| {
        let venv = repo_root().join("sender").join(".venv");
        let p = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
        if p.is_file() { p } else { PathBuf::from(if cfg!(windows) { "python" } else { "python3" }) }
    })
}

/// Sends one command to the pyatv sidecar, (re)starting it if needed. Replies arrive as "sender-event".
/// If the sidecar dies, "sender-exit" is emitted so the UI can fail outstanding requests and retry.
#[tauri::command]
async fn sender_send(app: AppHandle, command: Value) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let sidecars = &state.sidecars;
        let mut guard = sidecars.sender.lock().unwrap();
        if guard.as_ref().is_some_and(|(_, s)| !s.is_running()) {
            *guard = None;
        }
        if guard.is_none() {
            let generation = sidecars.sender_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let script = repo_root().join("sender").join("oap_sender.py");
            let (events, exits) = (app.clone(), app.clone());
            let sidecar = SenderSidecar::start(
                sender_python(),
                script,
                move |event| {
                    let _ = events.emit("sender-event", &event);
                },
                move |exit| {
                    if exit.requested {
                        return;
                    }
                    let state = exits.state::<AppState>();
                    let mut current = state.sidecars.sender.lock().unwrap();
                    if current.as_ref().is_some_and(|(g, _)| *g == generation) {
                        *current = None;
                    }
                    drop(current);
                    let code = exit.code.map(|c| format!(" (exit code {c})")).unwrap_or_default();
                    let _ = exits.emit("sender-exit", json!({ "message": format!("The sender stopped unexpectedly{code}. Try again.") }));
                },
            )
            .map_err(|e| format!("Could not start the sender: {e}"))?;
            *guard = Some((generation, sidecar));
        }
        let result = guard.as_ref().unwrap().1.send(&command);
        result.map_err(|e| {
            *guard = None;
            format!("The sender isn't running: {e}. Try again.")
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The engine exe the doctor checks firewall rules for (without the `\\?\` prefix Windows APIs add).
fn engine_path_for_doctor(status: &ReceiverStatus) -> Option<String> {
    let path = status.engine_path.clone()?;
    Some(path.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(path))
}

/// Runs the network doctor in the background and publishes its findings ("doctor-findings").
fn run_doctor(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let Some(engine) = engine_path_for_doctor(&state.status.lock().unwrap()) else { return };
        let ip = netwatch::primary_ip().map(|i| i.to_string());
        let own = state.sidecars.engine_pid();
        let Some(snapshot) = doctor::probe(&engine, ip.as_deref(), own) else { return };
        let findings = doctor::diagnose(&snapshot);
        *state.findings.lock().unwrap() = findings.clone();
        let _ = app.emit("doctor-findings", &findings);
    });
}

#[tauri::command]
fn doctor_findings(app: AppHandle) -> Vec<doctor::Finding> {
    app.state::<AppState>().findings.lock().unwrap().clone()
}

#[tauri::command]
fn doctor_check(app: AppHandle) {
    run_doctor(&app);
}

/// One-click fixes. Each needs the user's click; the firewall one also shows Windows' admin prompt.
#[tauri::command]
async fn doctor_fix(app: AppHandle, kind: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        match kind.as_str() {
            // Changing the network type is a Windows setting the user makes; open the right page.
            "public_network" => {
                std::process::Command::new("explorer.exe").arg("ms-settings:network-status").spawn().map_err(|e| e.to_string())?;
            }
            "firewall_blocks" => {
                let state = app.state::<AppState>();
                let engine = engine_path_for_doctor(&state.status.lock().unwrap()).ok_or("The receiver isn't set up yet.")?;
                let inner = doctor::firewall_fix_script(&engine).replace('\'', "''");
                let launcher = format!(
                    "Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-Command','{inner}'"
                );
                let status = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &launcher])
                    .status()
                    .map_err(|e| e.to_string())?;
                if !status.success() {
                    return Err("Windows didn't allow the change (the admin prompt was declined).".into());
                }
            }
            other => return Err(format!("No fix for {other}.")),
        }
        run_doctor(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A network change or wake restarts only a receiver the user has on; one that is off stays off.
fn settings_to_restart(status: &ReceiverStatus) -> Option<ReceiverSettings> {
    status.wanted.then(|| status.settings.clone()).flatten()
}

/// Restarts the receiver after sleep or a network change, so iPhones on the (new) network can find it.
fn spawn_network_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        const TICK: Duration = Duration::from_secs(3);
        let mut ip = netwatch::primary_ip();
        let mut last = SystemTime::now();
        loop {
            std::thread::sleep(TICK);
            let now = SystemTime::now();
            let sample = netwatch::Sample {
                wall_gap: now.duration_since(last).unwrap_or(TICK),
                primary_ip: netwatch::primary_ip(),
            };
            last = now;
            let decision = netwatch::decide(ip, sample);
            if sample.primary_ip.is_some() {
                ip = sample.primary_ip;
            }
            let Some(reason) = decision else { continue };
            let settings = settings_to_restart(&app.state::<AppState>().status.lock().unwrap());
            if let Some(settings) = settings {
                eprintln!("open-airplay: restarting the receiver ({reason:?})");
                let _ = start_receiver_inner(&app, settings);
            }
        }
    });
}

/// Tray left-click: toggle the compact popover just above (or below) the tray icon.
fn toggle_popover(window: &WebviewWindow, icon_rect: tauri::Rect) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    show_popover_at(window, icon_rect);
}

/// Opens the popover by the tray icon, or at the bottom-right of the work area if the icon can't be found.
fn show_popover(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    match app.tray_by_id("main").and_then(|t| t.rect().ok().flatten()) {
        Some(rect) => show_popover_at(&window, rect),
        None => {
            if let (Ok(size), Ok(Some(monitor))) = (window.outer_size(), window.primary_monitor()) {
                let area = monitor.work_area();
                let gap = (12.0 * window.scale_factor().unwrap_or(1.0)) as i32;
                let x = area.position.x + area.size.width as i32 - size.width as i32 - gap;
                let y = area.position.y + area.size.height as i32 - size.height as i32 - gap;
                let _ = window.set_position(PhysicalPosition::new(x, y));
            }
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

fn show_popover_at(window: &WebviewWindow, icon_rect: tauri::Rect) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let icon_pos: PhysicalPosition<f64> = icon_rect.position.to_physical(scale);
    let icon_size: tauri::PhysicalSize<f64> = icon_rect.size.to_physical(scale);
    if let (Ok(size), Ok(Some(monitor))) = (window.outer_size(), window.current_monitor().or_else(|_| window.primary_monitor())) {
        let (mw, mh) = (monitor.size().width as f64, monitor.size().height as f64);
        let (mx, my) = (monitor.position().x as f64, monitor.position().y as f64);
        let (w, h) = (size.width as f64, size.height as f64);
        let gap = 8.0 * scale;
        let x = (icon_pos.x + icon_size.width / 2.0 - w / 2.0).clamp(mx + gap, mx + mw - w - gap);
        // Taskbar at the bottom (usual): open above the icon; otherwise below it.
        let y = if icon_pos.y > my + mh / 2.0 { icon_pos.y - h - gap } else { icon_pos.y + icon_size.height + gap };
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn run() {
    let app = tauri::Builder::default()
        // A second launch focuses the running app instead of starting a rival receiver on the same ports.
        // It never changes receiving; a stray boot launch while running does nothing.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !launch::is_autostart(&args) {
                show_popover(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec![launch::AUTOSTART_ARG])))
        .manage(AppState::default())
        .setup(|app| {
            // Load saved prefs before anything reads them.
            {
                let state = app.state::<AppState>();
                let path = app.path().app_config_dir().ok().map(|d| d.join("settings.json"));
                let prefs = path.as_deref().map(|p| AppPrefs::load(p, &default_name())).unwrap_or_else(|| {
                    let mut p = AppPrefs::default();
                    p.receiver.name = default_name();
                    p
                });
                *state.prefs.lock().unwrap() = prefs;
                *state.prefs_path.lock().unwrap() = path;
                let mut offered = false;
                let prefs = state.update_prefs(|p| offered = p.offer_start_at_login());
                let result = if offered {
                    apply_start_at_login(app.handle(), true)
                } else if prefs.start_at_login {
                    refresh_start_at_login(app.handle())
                } else {
                    Ok(())
                };
                if let Err(e) = result {
                    eprintln!("open-airplay: {e}");
                }
            }
            let launch = launch::plan(&std::env::args().collect::<Vec<_>>(), &app.state::<AppState>().prefs.lock().unwrap());
            let show = MenuItem::with_id(app, "show", "Open open-airplay", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("bundle icon"))
                .tooltip("open-airplay")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            toggle_popover(&w, rect);
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            spawn_network_watcher(app.handle().clone());

            // A boot launch shows the off state in the tray and nothing else. A manual launch opens the
            // panel and, if the switch was left on, starts receiving so the PC shows up without a click.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let off = receiver_status(handle.clone());
                publish(&handle, &off);
                if launch.show_panel {
                    show_popover(&handle);
                }
                if !launch.start_receiving {
                    return;
                }
                let saved = handle.state::<AppState>().prefs.lock().unwrap().receiver.clone();
                if let Err(e) = start_receiver_inner(&handle, saved) {
                    let state = handle.state::<AppState>();
                    let snapshot = {
                        let mut st = state.status.lock().unwrap();
                        st.wanted = true;
                        st.last_error = Some(e);
                        st.phase = phase(&st);
                        st.clone()
                    };
                    publish(&handle, &snapshot);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![receiver_status, default_settings, get_prefs, set_prefs, show_video_window, doctor_findings, doctor_check, doctor_fix, receiver_start, receiver_stop, sender_send])
        .build(tauri::generate_context!())
        .expect("error while building open-airplay");

    // Tauri's run loop ends the process without running destructors, so stop and reap the sidecars
    // explicitly on every way out (tray Quit, closing the last window, OS shutdown).
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<AppState>().sidecars.shutdown();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(advertised: bool) -> ReceiverStatus {
        ReceiverStatus { wanted: true, running: true, advertised, ..Default::default() }
    }

    #[test]
    fn a_network_change_restarts_only_a_receiver_that_is_on() {
        let settings = Some(ReceiverSettings { name: "Den".into(), pin_mode: false, h265: false });
        let off = ReceiverStatus { settings: settings.clone(), ..Default::default() };
        assert_eq!(settings_to_restart(&off), None);
        assert_eq!(settings_to_restart(&ReceiverStatus::default()), None);
        let on = ReceiverStatus { wanted: true, settings: settings.clone(), ..Default::default() };
        assert_eq!(settings_to_restart(&on), settings);
    }

    #[test]
    fn the_doctor_checks_the_plain_engine_path() {
        let s = |p: &str| ReceiverStatus { engine_path: Some(p.into()), ..Default::default() };
        assert_eq!(engine_path_for_doctor(&s(r"\\?\C:\Program Files\open-airplay\engine\bin\uxplay.exe")).unwrap(), r"C:\Program Files\open-airplay\engine\bin\uxplay.exe");
        assert_eq!(engine_path_for_doctor(&s(r"C:\dev\uxplay.exe")).unwrap(), r"C:\dev\uxplay.exe");
        assert_eq!(engine_path_for_doctor(&ReceiverStatus::default()), None);
    }

    #[test]
    fn the_tray_tooltip_says_when_receiving_is_off() {
        assert_eq!(tray_text(&ReceiverStatus::default()), "open-airplay: receiving is off");
    }

    #[test]
    fn tray_shows_receiving_during_a_session_and_problem_on_error() {
        assert_eq!(tray_look(Phase::Ready), TrayLook::Normal);
        assert_eq!(tray_look(Phase::Stopped), TrayLook::Normal);
        assert_eq!(tray_look(Phase::Connecting), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Mirroring), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Pin), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Error), TrayLook::Problem);
    }

    #[test]
    fn phase_follows_the_receiver_lifecycle() {
        assert_eq!(phase(&ReceiverStatus::default()), Phase::Stopped);
        assert_eq!(phase(&ReceiverStatus { wanted: true, ..Default::default() }), Phase::Starting);
        assert_eq!(phase(&running(false)), Phase::Starting);
        assert_eq!(phase(&running(true)), Phase::Ready);
        let error = ReceiverStatus { wanted: true, last_error: Some("boom".into()), ..Default::default() };
        assert_eq!(phase(&error), Phase::Error);
    }

    #[test]
    fn a_probe_connection_is_not_a_session() {
        // Other Apple devices open short connections to query the receiver; that's still "ready".
        let mut s = running(true);
        apply_event(&mut s, &SupervisorEvent::Engine { event: EngineEvent::OpenConnections { count: 1 } });
        assert_eq!(s.phase, Phase::Ready);
    }

    #[test]
    fn a_session_goes_connecting_then_mirroring_then_back_to_ready() {
        let mut s = running(true);
        let engine = |event| SupervisorEvent::Engine { event };
        apply_event(&mut s, &engine(EngineEvent::OpenConnections { count: 1 }));
        apply_event(
            &mut s,
            &engine(EngineEvent::ConnectionRequest { name: "Parsa's iPhone".into(), model: "iPhone16,1".into(), device_id: "x".into() }),
        );
        assert_eq!(s.phase, Phase::Connecting);
        apply_event(&mut s, &engine(EngineEvent::Video { width: 886, height: 1920 }));
        assert_eq!(s.phase, Phase::Mirroring);
        assert!(s.mirroring_since_ms.is_some());
        apply_event(&mut s, &engine(EngineEvent::OpenConnections { count: 0 }));
        assert_eq!(s.phase, Phase::Ready);
        assert_eq!((s.last_device.clone(), s.video), (None, None));
    }

    #[test]
    fn pin_shows_until_video_starts() {
        let mut s = running(true);
        apply_event(&mut s, &SupervisorEvent::Engine { event: EngineEvent::Pin { code: "4821".into() } });
        assert_eq!(s.phase, Phase::Pin);
        apply_event(&mut s, &SupervisorEvent::Engine { event: EngineEvent::Video { width: 1920, height: 886 } });
        assert_eq!(s.phase, Phase::Mirroring);
    }

    #[test]
    fn an_unexpected_exit_is_an_error_but_a_requested_stop_is_not() {
        let mut s = running(true);
        apply_event(&mut s, &SupervisorEvent::Exited { code: Some(1) });
        assert_eq!(s.phase, Phase::Error);
        let mut stopped = ReceiverStatus { wanted: false, running: true, ..Default::default() };
        apply_event(&mut stopped, &SupervisorEvent::Exited { code: Some(1) });
        assert_eq!(stopped.phase, Phase::Stopped);
    }

    fn pid_alive(pid: u32) -> bool {
        if cfg!(windows) {
            let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
        } else {
            std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false)
        }
    }

    #[test]
    fn shutdown_kills_and_reaps_the_receiver_engine() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cmd = EngineCommand {
            program: if cfg!(windows) { "python".into() } else { "python3".into() },
            args: vec!["-c".into(), "import time\ntime.sleep(30)".into()],
            path_prepend: vec![],
            env: vec![],
            restart_delay: Duration::from_secs(60),
        };
        let sup = Supervisor::start(cmd, move |e| {
            let _ = tx.send(e);
        });
        let SupervisorEvent::Started { pid } = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("expected Started");
        };
        let sidecars = Sidecars::default();
        sidecars.replace_supervisor(Some(sup));
        assert!(pid_alive(pid));
        sidecars.shutdown();
        assert!(!pid_alive(pid), "engine {pid} still running after shutdown");
    }
}
