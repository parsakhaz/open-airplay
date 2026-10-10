pub mod engine;
pub mod sender;
pub mod supervisor;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};

use engine::{EngineEvent, ReceiverSettings};
use sender::SenderSidecar;
use supervisor::{EngineCommand, Supervisor, SupervisorEvent};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReceiverStatus {
    pub running: bool,
    pub engine_path: Option<String>,
    pub settings: Option<ReceiverSettings>,
    pub advertised: bool,
    pub connections: u32,
    pub last_device: Option<String>,
    pub pin: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct AppState {
    supervisor: Mutex<Option<Supervisor>>,
    status: Arc<Mutex<ReceiverStatus>>,
    sender: Mutex<Option<SenderSidecar>>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) { format!("{stem}.exe") } else { stem.to_string() }
}

/// OAP_ENGINE, then the dev build (engine/dist), then the bundled resource.
fn find_engine(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("OAP_ENGINE").map(PathBuf::from).into_iter().collect();
    candidates.push(repo_root().join("engine").join("dist").join(exe_name("uxplay")));
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join("engine").join(exe_name("uxplay")));
    }
    candidates.into_iter().find(|p| p.is_file())
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

fn apply_event(status: &mut ReceiverStatus, event: &SupervisorEvent) {
    match event {
        SupervisorEvent::Started { .. } => {
            status.running = true;
            status.last_error = None;
        }
        SupervisorEvent::Exited { code } => {
            status.running = false;
            status.advertised = false;
            status.connections = 0;
            status.last_error = Some(format!("engine exited (code {code:?}); restarting"));
        }
        SupervisorEvent::SpawnFailed { message } => {
            status.running = false;
            status.last_error = Some(format!("could not start engine: {message}"));
        }
        SupervisorEvent::Engine { event } => match event {
            EngineEvent::Advertised => status.advertised = true,
            EngineEvent::ConnectionRequest { name, model, .. } => status.last_device = Some(format!("{name} ({model})")),
            EngineEvent::OpenConnections { count } => {
                status.connections = *count;
                if *count == 0 {
                    status.pin = None;
                }
            }
            EngineEvent::Pin { code } => status.pin = Some(code.clone()),
            EngineEvent::ClientLost => status.last_error = Some("lost connection with the sender".into()),
            EngineEvent::Error { message } => status.last_error = Some(message.clone()),
        },
        SupervisorEvent::Log { .. } => {}
    }
}

fn tray_text(status: &ReceiverStatus) -> String {
    let name = status.settings.as_ref().map(|s| s.name.as_str()).unwrap_or("this PC");
    match (status.running, status.advertised, status.connections) {
        (_, _, n) if n > 0 => format!("open-airplay: receiving on '{name}'"),
        (true, true, _) => format!("open-airplay: visible as '{name}'"),
        (true, false, _) => "open-airplay: starting…".into(),
        _ => "open-airplay: receiver stopped".into(),
    }
}

fn start_receiver_inner(app: &AppHandle, state: &AppState, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    let engine = find_engine(app).ok_or("UxPlay engine not found. Run engine/build-engine.sh (see README).")?;
    if let Some(old) = state.supervisor.lock().unwrap().take() {
        old.stop();
    }
    let (path_prepend, mut env) = engine_runtime();
    env.extend(settings.engine_env(find_window_icon(app).as_deref()));
    {
        let mut st = state.status.lock().unwrap();
        *st = ReceiverStatus { engine_path: Some(engine.display().to_string()), settings: Some(settings.clone()), ..Default::default() };
    }
    let cmd = EngineCommand { program: engine, args: settings.engine_args(), path_prepend, env, restart_delay: Duration::from_secs(2) };
    let (status, handle) = (state.status.clone(), app.clone());
    // Every engine line and lifecycle event goes to <app log dir>/engine.log for diagnosis.
    let log = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("engine.log")).ok()
    });
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
        if let Some(tray) = handle.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(tray_text(&snapshot)));
        }
        let _ = handle.emit("receiver-event", &event);
        let _ = handle.emit("receiver-status", &snapshot);
    });
    *state.supervisor.lock().unwrap() = Some(sup);
    Ok(state.status.lock().unwrap().clone())
}

#[tauri::command]
fn receiver_status(state: State<AppState>) -> ReceiverStatus {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
fn default_settings() -> ReceiverSettings {
    ReceiverSettings { name: default_name(), pin_mode: false, h265: false }
}

#[tauri::command]
fn receiver_start(app: AppHandle, state: State<AppState>, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    start_receiver_inner(&app, &state, settings)
}

#[tauri::command]
fn receiver_stop(state: State<AppState>) -> ReceiverStatus {
    if let Some(sup) = state.supervisor.lock().unwrap().take() {
        sup.stop();
    }
    let mut st = state.status.lock().unwrap();
    st.running = false;
    st.advertised = false;
    st.connections = 0;
    st.last_error = None;
    st.clone()
}

/// Sends one command to the pyatv sidecar, starting it on first use. Replies arrive as "sender-event".
#[tauri::command]
fn sender_send(app: AppHandle, state: State<AppState>, command: Value) -> Result<(), String> {
    let mut guard = state.sender.lock().unwrap();
    if guard.is_none() {
        let python = std::env::var_os("OAP_PYTHON").map(PathBuf::from).unwrap_or_else(|| {
            let venv = repo_root().join("sender").join(".venv");
            let p = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
            if p.is_file() { p } else { PathBuf::from(if cfg!(windows) { "python" } else { "python3" }) }
        });
        let script = repo_root().join("sender").join("oap_sender.py");
        let handle = app.clone();
        let sidecar = SenderSidecar::start(python, script, move |event| {
            let _ = handle.emit("sender-event", &event);
        })
        .map_err(|e| format!("could not start sender: {e}"))?;
        *guard = Some(sidecar);
    }
    guard.as_ref().unwrap().send(&command).map_err(|e| {
        *guard = None;
        format!("sender sidecar is gone: {e}")
    })
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "Open open-airplay", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("bundle icon"))
                .tooltip("open-airplay")
                .menu(&menu)
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

            // Start receiving right away so the PC shows up in Screen Mirroring without a click.
            let handle = app.handle().clone();
            let state = app.state::<AppState>();
            if let Err(e) = start_receiver_inner(&handle, &state, default_settings()) {
                state.status.lock().unwrap().last_error = Some(e);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![receiver_status, default_settings, receiver_start, receiver_stop, sender_send])
        .run(tauri::generate_context!())
        .expect("error while running open-airplay");
}
