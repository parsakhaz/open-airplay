pub mod added;
pub mod dacp;
pub mod doctor;
pub mod engine;
pub mod engine_log;
pub mod launch;
pub mod mirror;
pub mod mirror_options;
pub mod mirror_targets;
pub mod netwatch;
pub mod panel;
pub mod prefs;
pub mod process_guard;
pub mod sender;
pub mod speakers;
pub mod status;
pub mod supervisor;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent};

use engine::{BlockedDevice, EngineEvent, ReceiverSettings};
use prefs::AppPrefs;
use sender::SenderSidecar;
use status::{apply_event, device_name, end_session, session_live, Phase, ReceiverStatus, StatusHub};
use supervisor::{EngineCommand, Supervisor, SupervisorEvent};

/// The processes the app owns. `shutdown` stops and reaps all of them; it runs on every exit path.
#[derive(Default)]
pub struct Sidecars {
    supervisor: Mutex<Option<Supervisor>>,
    /// (generation, sidecar): the generation lets a late exit notice skip a newer sidecar.
    sender: Mutex<Option<(u64, SenderSidecar)>>,
    sender_generation: AtomicU64,
    /// The running cast sidecar's ready line ({"event": "ready", "pyatv": ...}), once it has printed it.
    sender_ready: Mutex<Option<Value>>,
    /// Devices this PC is casting to right now (identifiers), from the sidecar's playing and final events.
    casting: Mutex<std::collections::BTreeSet<String>>,
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

    /// True while this PC is casting to at least one device.
    pub fn is_casting(&self) -> bool {
        !self.casting.lock().unwrap().is_empty()
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
    status: StatusHub,
    /// Held while a status change is made and published, so pushes leave in sequence order.
    publish_order: Mutex<()>,
    prefs: Mutex<AppPrefs>,
    /// Where prefs are saved; set during setup (None in tests or if there's no config folder).
    prefs_path: Mutex<Option<PathBuf>>,
    panel: PanelState,
    /// Mirroring this PC to a TV; set during setup.
    mirror: std::sync::OnceLock<mirror::Mirror>,
    lifecycle: Lifecycle,
}

/// Turning the receiver on, off or restarting it happens one at a time. Every start and stop
/// takes a new generation, so a restart scheduled earlier (settings saved during a session, a
/// network change) can tell that receiving was turned off or restarted since, and stand down.
#[derive(Default)]
struct Lifecycle {
    lock: Mutex<()>,
    generation: AtomicU64,
}

impl Lifecycle {
    /// The current generation, for a restart that will run later.
    fn ticket(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Starts a start or stop: holds the lock and makes every earlier ticket stale.
    fn begin(&self) -> std::sync::MutexGuard<'_, ()> {
        let guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.generation.fetch_add(1, Ordering::SeqCst);
        guard
    }

    /// Runs `restart` only if nothing started or stopped the receiver since `ticket` was taken.
    /// It runs under the lock, so a stop can't slip in between the check and the restart.
    fn restart_if_current<T>(&self, ticket: u64, restart: impl FnOnce() -> T) -> Option<T> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        (self.generation.load(Ordering::SeqCst) == ticket).then(|| {
            self.generation.fetch_add(1, Ordering::SeqCst);
            restart()
        })
    }
}

/// The tray panel's placement and dismissal bookkeeping (see panel.rs for the rules).
#[derive(Default)]
struct PanelState {
    visible: AtomicBool,
    /// The tray icon's rectangle the last time it was clicked (physical px).
    anchor: Mutex<Option<panel::Rect>>,
    /// The content's height in logical px, as the UI last reported it.
    content_height: Mutex<Option<f64>>,
    last_dismissed: Mutex<Option<Instant>>,
    /// Until when a window the app opened may take the focus without closing the panel.
    grace_until: Mutex<Option<Instant>>,
    /// Actions in progress that bring up their own windows (the firewall fix's UAC prompt).
    holds: AtomicUsize,
}

impl PanelState {
    fn in_grace(&self) -> bool {
        self.holds.load(Ordering::SeqCst) > 0 || self.grace_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
    }

    fn allow_own_window(&self) {
        self.grace_for(panel::OWN_WINDOW_GRACE);
    }

    fn grace_for(&self, d: std::time::Duration) {
        let until = Instant::now() + d;
        let mut grace = self.grace_until.lock().unwrap();
        if grace.is_none_or(|t| t < until) {
            *grace = Some(until);
        }
    }
}

impl AppState {
    /// The app's config folder, where the receiver keeps its files (None in tests).
    fn data_dir(&self) -> Option<PathBuf> {
        self.prefs_path.lock().unwrap().as_ref().and_then(|p| p.parent().map(PathBuf::from))
    }

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

/// OAP_ENGINE wins (an explicit override, e.g. the test fake engine), then the installed engine,
/// then the dev build (engine/dist).
fn find_engine(app: &AppHandle) -> Option<(PathBuf, EngineSource)> {
    let mut candidates: Vec<(PathBuf, EngineSource)> = Vec::new();
    if let Some(p) = std::env::var_os("OAP_ENGINE") {
        candidates.push((PathBuf::from(p), EngineSource::Dev));
    }
    if let Ok(res) = app.path().resource_dir() {
        candidates.push((res.join("engine").join("bin").join(exe_name("uxplay")), EngineSource::Bundled));
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
        Phase::Audio => format!("open-airplay: playing audio on '{name}'"),
        Phase::Video => "open-airplay: playing a video".into(),
        Phase::Connecting => "open-airplay: connecting…".into(),
        Phase::Pin => "open-airplay: enter the code on your device".into(),
        Phase::Ready => format!("open-airplay: ready to receive as '{name}'"),
        Phase::Starting => "open-airplay: starting…".into(),
        Phase::Restarting => "open-airplay: restarting…".into(),
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
        Phase::Connecting | Phase::Mirroring | Phase::Audio | Phase::Video | Phase::Pin => TrayLook::Receiving,
        Phase::Error => TrayLook::Problem,
        Phase::Stopped | Phase::Starting | Phase::Restarting | Phase::Ready => TrayLook::Normal,
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

/// What the tray menu offers besides Quit: Disconnect while a phone is connected to this PC, and
/// Stop sending while this PC mirrors or casts to any device (it stops them all).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TrayItems {
    disconnect: bool,
    stop_sending: bool,
}

impl TrayItems {
    fn ids(self) -> Vec<(&'static str, &'static str)> {
        let mut items = Vec::new();
        if self.disconnect {
            items.push(("disconnect", "Disconnect"));
        }
        if self.stop_sending {
            items.push(("stop_sending", "Stop sending"));
        }
        items.push(("quit", "Quit"));
        items
    }
}

fn tray_menu(app: &AppHandle, items: TrayItems) -> tauri::Result<Menu<tauri::Wry>> {
    let items = items.ids().into_iter().map(|(id, text)| MenuItem::with_id(app, id, text, true, None::<&str>)).collect::<tauri::Result<Vec<_>>>()?;
    let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect();
    Menu::with_items(app, &refs)
}

static TRAY_DISCONNECT: AtomicBool = AtomicBool::new(false);
static TRAY_MIRRORING: AtomicBool = AtomicBool::new(false);
static TRAY_CASTING: AtomicBool = AtomicBool::new(false);

/// Rebuilds the tray menu when what it offers changed. Waits for the main thread, so never call it there.
fn set_tray_menu(app: &AppHandle, tray: &tauri::tray::TrayIcon) {
    static SHOWN: Mutex<TrayItems> = Mutex::new(TrayItems { disconnect: false, stop_sending: false });
    let mut shown = SHOWN.lock().unwrap();
    let want = TrayItems { disconnect: TRAY_DISCONNECT.load(Ordering::SeqCst), stop_sending: TRAY_MIRRORING.load(Ordering::SeqCst) || TRAY_CASTING.load(Ordering::SeqCst) };
    if *shown == want {
        return;
    }
    if let Ok(menu) = tray_menu(app, want) {
        let _ = tray.set_menu(Some(menu));
        *shown = want;
    }
}

/// Mirroring or casting started or ended: offer Stop sending while either sends. Callers hold the
/// mirror's or the cast sidecar's lock, so the menu is rebuilt on its own thread.
fn set_tray_sending(app: &AppHandle, flag: &AtomicBool, sending: bool) {
    if flag.swap(sending, Ordering::SeqCst) == sending {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(tray) = app.tray_by_id("main") {
            set_tray_menu(&app, &tray);
        }
    });
}

fn publish(app: &AppHandle, snapshot: &ReceiverStatus) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tray_text(snapshot)));
        set_tray_look(app, &tray, tray_look(snapshot.phase));
        TRAY_DISCONNECT.store(session_live(snapshot), Ordering::SeqCst);
        set_tray_menu(app, &tray);
    }
    let _ = app.emit("receiver-status", snapshot);
}

/// Changes the receiver status and publishes it if anything the UI shows changed. Returns the
/// current snapshot. Never call this on the main thread: updating the tray waits for it.
fn update_status(app: &AppHandle, change: impl FnOnce(&mut ReceiverStatus)) -> ReceiverStatus {
    let state = app.state::<AppState>();
    let _order = state.publish_order.lock().unwrap();
    match state.status.update(change) {
        Some(snapshot) => {
            publish(app, &snapshot);
            snapshot
        }
        None => state.status.snapshot(),
    }
}

/// Starts (or restarts) receiving with `settings`. `reason` says why it restarts on its own
/// ("the network changed"); None when the user turned it on.
fn start_receiver_inner(app: &AppHandle, settings: ReceiverSettings, reason: Option<&str>) -> Result<ReceiverStatus, String> {
    let state = app.state::<AppState>();
    let _lifecycle = state.lifecycle.begin();
    start_receiver_locked(app, settings, reason)
}

/// `start_receiver_inner` for a caller that already holds the lifecycle lock.
fn start_receiver_locked(app: &AppHandle, settings: ReceiverSettings, reason: Option<&str>) -> Result<ReceiverStatus, String> {
    let state = app.state::<AppState>();
    state.update_prefs(|p| p.receiver = settings.clone());
    let (engine, source) = find_engine(app).ok_or("Part of open-airplay is missing, so it can't receive. Reinstall open-airplay.")?;
    state.sidecars.replace_supervisor(None);
    let (path_prepend, mut env) = match source {
        EngineSource::Dev => engine_runtime(),
        EngineSource::Bundled => (vec![], vec![]),
    };
    env.extend(settings.engine_env(find_window_icon(app).as_deref()));
    update_status(app, |st| {
        *st = ReceiverStatus {
            wanted: true,
            engine_path: Some(engine.display().to_string()),
            settings: Some(settings.clone()),
            restart_reason: reason.map(str::to_string),
            // A note about what just happened ("Blocked …") outlives the restart it caused.
            notice: st.notice.take(),
            ..Default::default()
        }
    });
    let mut args = engine_settings(settings, &speakers::list()).engine_args(state.data_dir().as_deref());
    if let Some(dir) = audio_dir(app) {
        args.extend(engine::audio_args(&dir));
    }
    let cmd = EngineCommand { program: engine, args, path_prepend, env, restart_delay: Duration::from_secs(2) };
    let handle = app.clone();
    // Every engine line and lifecycle event goes to <app log dir>/engine.log (rotated, see engine_log) for diagnosis.
    let log = app.path().app_log_dir().ok().and_then(|dir| engine_log::shared(&dir));
    // Runs on the supervisor's delivery thread, never on a thread that waits for the supervisor.
    // Every line goes to the log; only real status changes reach the UI.
    let sup = Supervisor::start(cmd, move |event| {
        if let Some(log) = log.as_ref() {
            log.write_line(&match &event {
                SupervisorEvent::Log { line } => line.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            });
        }
        if matches!(event, SupervisorEvent::Log { .. }) {
            return;
        }
        let snapshot = update_status(&handle, |st| apply_event(st, &event));
        after_event(&handle, &snapshot);
        if matches!(event, SupervisorEvent::Engine { event: EngineEvent::Advertised }) {
            run_doctor(&handle);
        }
    });
    state.sidecars.replace_supervisor(Some(sup));
    Ok(state.status.snapshot())
}

/// Where the engine writes cover art and the phone's remote-control id (it needs the folder to exist).
fn audio_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_local_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The settings the engine runs with: a chosen speaker that isn't plugged in plays on the Windows
/// default instead (the engine can't open it), and comes back when it is.
fn engine_settings(mut settings: ReceiverSettings, available: &[speakers::Speaker]) -> ReceiverSettings {
    if settings.speaker.as_ref().is_some_and(|id| !available.iter().any(|s| &s.id == id)) {
        settings.speaker = None;
    }
    settings
}

fn stop_receiver_inner(app: &AppHandle) -> ReceiverStatus {
    let state = app.state::<AppState>();
    let _lifecycle = state.lifecycle.begin();
    // Unwanted before the engine goes: its exit must not schedule the restart for saved settings.
    update_status(app, |st| {
        st.wanted = false;
        st.settings_pending = false;
    });
    state.sidecars.replace_supervisor(None);
    update_status(app, |st| {
        st.wanted = false;
        st.running = false;
        st.advertised = false;
        st.last_error = None;
        end_session(st);
    })
}

/// Follow-ups to a status change that need the receiver restarted or stopped (off this thread:
/// replacing the supervisor joins its worker).
fn after_event(app: &AppHandle, snapshot: &ReceiverStatus) {
    let app = app.clone();
    if snapshot.phase == Phase::Error && app.state::<AppState>().sidecars.engine_pid().is_none() {
        // Too many failed restarts: stop trying until the user turns receiving back on.
        std::thread::spawn(move || {
            let state = app.state::<AppState>();
            if state.status.snapshot().phase == Phase::Error {
                state.sidecars.replace_supervisor(None);
            }
        });
    } else if snapshot.settings_pending && snapshot.wanted && !session_live(snapshot) {
        // The session ended: apply the settings saved during it. Taking the flag here schedules one
        // restart, which stands down if receiving is turned off (or restarted) before it runs.
        update_status(&app, |st| st.settings_pending = false);
        let ticket = app.state::<AppState>().lifecycle.ticket();
        std::thread::spawn(move || {
            let state = app.state::<AppState>();
            let settings = state.prefs.lock().unwrap().receiver.clone();
            let started = state.lifecycle.restart_if_current(ticket, || {
                state.status.snapshot().wanted.then(|| start_receiver_locked(&app, settings, None))
            });
            if let Some(Some(Err(e))) = started {
                update_status(&app, |st| st.last_error = Some(e));
            }
        });
    }
}

/// Ends only the current incoming session: the engine drops the phone and keeps advertising,
/// so the PC stays in the phone's Screen Mirroring list (engine patch 0003).
fn disconnect_inner(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let sup = state.sidecars.supervisor.lock().unwrap();
    let sup = sup.as_ref().ok_or("Receiving is off.")?;
    sup.send_line("disconnect").map_err(|_| "Couldn't disconnect. Try turning receiving off and on.".to_string())
}

/// Play/pause, next or previous on the phone playing audio to this PC.
#[tauri::command]
async fn receiver_control(app: AppHandle, command: dacp::Command) -> Result<(), String> {
    let remote = app.state::<AppState>().status.snapshot().remote.ok_or("The phone doesn't accept controls from this PC.")?;
    tauri::async_runtime::spawn_blocking(move || dacp::control(&remote, command)).await.map_err(|e| e.to_string())?
}

/// The current cover art as a data: URL, or None when there is none.
#[tauri::command]
fn receiver_art(app: AppHandle) -> Option<String> {
    let bytes = std::fs::read(app.path().app_local_data_dir().ok()?.join(engine::ART_FILE)).ok()?;
    art_data_url(&bytes)
}

/// Cover art arrives as JPEG or PNG; the engine's placeholder (and anything else) shows no picture.
fn art_data_url(bytes: &[u8]) -> Option<String> {
    use base64::Engine as _;
    let mime = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) && !is_placeholder_png(bytes) {
        "image/png"
    } else {
        return None;
    };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// The engine writes a 1x1 PNG over the art file when a new stream starts.
fn is_placeholder_png(bytes: &[u8]) -> bool {
    let dim = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    dim(16) == Some(1) && dim(20) == Some(1)
}

#[tauri::command]
async fn receiver_disconnect(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || disconnect_inner(&app)).await.map_err(|e| e.to_string())?
}

/// What saving receiver settings should do to a receiver in this state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsChange {
    /// Receiving is off (or needs attention): just save; the switch uses them next time.
    SaveOnly,
    /// Receiving and idle: restart now so phones see the change.
    RestartNow,
    /// A phone is connected: restart when the session ends.
    AfterSession,
}

fn settings_change(status: &ReceiverStatus) -> SettingsChange {
    if !status.wanted || status.last_error.is_some() {
        SettingsChange::SaveOnly
    } else if session_live(status) {
        SettingsChange::AfterSession
    } else {
        SettingsChange::RestartNow
    }
}

/// Saves receiver settings as the user changes them. Never turns receiving on.
fn save_settings_inner(app: &AppHandle, mut settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    settings.name = settings.name.trim().to_string();
    if settings.name.is_empty() {
        return Err("Enter a name. iPhones show it in Screen Mirroring.".into());
    }
    app.state::<AppState>().update_prefs(|p| p.receiver = settings.clone());
    apply_settings(app, settings)
}

/// Puts saved settings into effect: now, after the live session, or next time receiving is turned on.
fn apply_settings(app: &AppHandle, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    let state = app.state::<AppState>();
    match settings_change(&state.status.snapshot()) {
        SettingsChange::RestartNow => start_receiver_inner(app, settings, None),
        SettingsChange::AfterSession => Ok(update_status(app, |st| st.settings_pending = true)),
        SettingsChange::SaveOnly => Ok(update_status(app, |st| st.settings = Some(settings))),
    }
}

#[tauri::command]
async fn receiver_save_settings(app: AppHandle, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    tauri::async_runtime::spawn_blocking(move || save_settings_inner(&app, settings)).await.map_err(|e| e.to_string())?
}

/// "Block this device": adds the connected device to the blocked list and ends its session. The
/// receiver restarts with the new list once the session has ended.
fn block_device_inner(app: &AppHandle) -> Result<ReceiverStatus, String> {
    let state = app.state::<AppState>();
    let snapshot = state.status.snapshot();
    let id = snapshot.device_id.clone().ok_or("No device is connected.")?;
    let name = device_name(&snapshot).unwrap_or("This device").to_string();
    let mut settings = state.prefs.lock().unwrap().receiver.clone();
    if !settings.blocked.iter().any(|d| d.id == id) {
        settings.blocked.push(BlockedDevice { id, name: name.clone() });
    }
    save_settings_inner(app, settings)?;
    if let Err(e) = disconnect_inner(app) {
        eprintln!("open-airplay: {e}");
    }
    Ok(update_status(app, |st| st.notice = Some(format!("Blocked {name}. You can unblock it in Settings."))))
}

#[tauri::command]
async fn receiver_block_device(app: AppHandle) -> Result<ReceiverStatus, String> {
    tauri::async_runtime::spawn_blocking(move || block_device_inner(&app)).await.map_err(|e| e.to_string())?
}

/// Forgets the devices that entered the PIN: each is asked for the code again next time.
fn forget_paired_devices_inner(app: &AppHandle) -> Result<ReceiverStatus, String> {
    let state = app.state::<AppState>();
    let dir = state.data_dir().ok_or("There are no remembered devices.")?;
    match std::fs::remove_file(dir.join(engine::PAIRED_DEVICES_FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Couldn't forget the devices: {e}")),
    }
    // The engine keeps the list in memory; restart it like a settings change.
    let settings = state.prefs.lock().unwrap().receiver.clone();
    apply_settings(app, settings)
}

#[tauri::command]
async fn receiver_forget_paired(app: AppHandle) -> Result<ReceiverStatus, String> {
    tauri::async_runtime::spawn_blocking(move || forget_paired_devices_inner(&app)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_speakers() -> Vec<speakers::Speaker> {
    speakers::list()
}

#[tauri::command]
async fn receiver_dismiss_notice(app: AppHandle) -> Result<ReceiverStatus, String> {
    tauri::async_runtime::spawn_blocking(move || update_status(&app, |st| st.notice = None)).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn receiver_status(app: AppHandle) -> ReceiverStatus {
    app.state::<AppState>().status.snapshot()
}

#[tauri::command]
fn default_settings(app: AppHandle) -> ReceiverSettings {
    app.state::<AppState>().prefs.lock().unwrap().receiver.clone()
}

/// Brings the engine's mirror window to the front (it belongs to the uxplay process). The panel stays open.
#[tauri::command]
fn show_video_window(app: AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowExW, GetWindowThreadProcessId, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        let state = app.state::<AppState>();
        let not_open = || "The video window isn't open.".to_string();
        let engine = state.sidecars.engine_pid().ok_or_else(not_open)?;
        let class: Vec<u16> = "OpenAirplayMirrorWindow".encode_utf16().chain(Some(0)).collect();
        unsafe {
            // Only our own engine's window: another open-airplay (say, the installed one) has the same class.
            let mut hwnd = std::ptr::null_mut();
            loop {
                hwnd = FindWindowExW(std::ptr::null_mut(), hwnd, class.as_ptr(), std::ptr::null());
                if hwnd.is_null() {
                    return Err(not_open());
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == engine {
                    break;
                }
            }
            state.panel.allow_own_window();
            ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(hwnd);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err("Not supported on this platform yet.".into())
    }
}

#[tauri::command]
fn get_prefs(app: AppHandle) -> AppPrefs {
    app.state::<AppState>().prefs.lock().unwrap().clone()
}

/// Saves the app-level prefs (receiver settings are saved by receiver_start) and applies start-at-login.
#[tauri::command]
fn set_prefs(app: AppHandle, start_at_login: bool) -> Result<AppPrefs, String> {
    let prefs = app.state::<AppState>().update_prefs(|p| p.start_at_login = start_at_login);
    apply_start_at_login(&app, prefs.start_at_login)?;
    Ok(prefs)
}

/// Saves the Advanced choices in the Mirror sheet: the defaults for every mirror.
#[tauri::command]
fn set_mirror_options(app: AppHandle, options: mirror_options::MirrorOptions) -> AppPrefs {
    app.state::<AppState>().update_prefs(|p| p.mirror = options)
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

/// Brings the Run entry in line with the pref at launch: an older entry gains --autostart, and one an
/// uninstall removed comes back. An entry the user turned off in Task Manager is left alone, since
/// enabling it would turn it back on.
fn refresh_start_at_login(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let entry = if app.autolaunch().is_enabled().unwrap_or(false) {
        launch::RunEntry::Enabled
    } else if run_value_exists(&app.package_info().name) {
        launch::RunEntry::DisabledByUser
    } else {
        launch::RunEntry::Missing
    };
    if launch::should_write_run_entry(true, entry) {
        apply_start_at_login(app, true)
    } else {
        Ok(())
    }
}

/// Whether the current user's Run key has a value with this name (enabled or not).
#[cfg(windows)]
fn run_value_exists(name: &str) -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, value) = (wide(r"Software\Microsoft\Windows\CurrentVersion\Run"), wide(name));
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
    };
    status == 0
}

/// Elsewhere the plugin's "enabled" is the whole story.
#[cfg(not(windows))]
fn run_value_exists(_name: &str) -> bool {
    false
}

/// Async + spawn_blocking: stopping the old engine joins a thread, which must never happen on the UI thread.
#[tauri::command]
async fn receiver_start(app: AppHandle, settings: ReceiverSettings) -> Result<ReceiverStatus, String> {
    app.state::<AppState>().update_prefs(|p| p.receiving = Some(true));
    tauri::async_runtime::spawn_blocking(move || start_receiver_inner(&app, settings, None)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn receiver_stop(app: AppHandle) -> Result<ReceiverStatus, String> {
    app.state::<AppState>().update_prefs(|p| p.receiving = Some(false));
    tauri::async_runtime::spawn_blocking(move || stop_receiver_inner(&app)).await.map_err(|e| e.to_string())
}

/// "Add device": checks what the user typed, then that something answers there. Errors are sentences for the user.
#[tauri::command]
async fn added_device_check(input: String, port: Option<String>) -> Result<added::Address, String> {
    let addr = added::parse_address_with_port(&input, port.as_deref()).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || added::check_reachable(&addr, Duration::from_secs(3)).map(|()| addr))
        .await
        .map_err(|e| e.to_string())?
}

/// Remembers an added device (or updates the one at that address) and returns the list.
#[tauri::command]
fn added_device_save(app: AppHandle, device: added::AddedDevice) -> Result<Vec<added::AddedDevice>, String> {
    let addr = added::parse_address(&device.host).map_err(|e| e.to_string())?;
    let device = added::AddedDevice { host: addr.host, ..device };
    Ok(app.state::<AppState>().update_prefs(|p| added::remember(&mut p.added_devices, device)).added_devices)
}

/// Forgets an added device and returns the list.
#[tauri::command]
fn added_device_remove(app: AppHandle, host: String) -> Vec<added::AddedDevice> {
    app.state::<AppState>().update_prefs(|p| {
        added::forget(&mut p.added_devices, &host);
    })
    .added_devices
}

fn sender_python() -> PathBuf {
    std::env::var_os("OAP_PYTHON").map(PathBuf::from).unwrap_or_else(|| {
        let venv = repo_root().join("sender").join(".venv");
        let p = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
        if p.is_file() { p } else { PathBuf::from(if cfg!(windows) { "python" } else { "python3" }) }
    })
}

fn find_sender_program(app: &AppHandle) -> Option<sender::SenderProgram> {
    let resources = app.path().resource_dir().ok();
    let override_exe = std::env::var_os("OAP_SENDER").map(PathBuf::from);
    sender::find_sender(resources.as_deref(), override_exe, sender_python(), &repo_root())
}

/// <app log dir>/sender.log: the cast sidecar's own log (its stderr) and the app's notes about it.
fn sender_log(app: &AppHandle) -> Option<std::fs::File> {
    let dir = app.path().app_log_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::OpenOptions::new().create(true).append(true).open(dir.join("sender.log")).ok()
}

fn sender_log_line(app: &AppHandle, line: &str) {
    if let Some(mut f) = sender_log(app) {
        use std::io::Write;
        let _ = writeln!(f, "open-airplay: {line}");
    }
}

const CASTING_UNAVAILABLE: &str = "Casting isn't available. Reinstall open-airplay to fix it.";
const CASTING_STOPPED: &str = "Casting stopped unexpectedly. Try again.";

type SenderSlot<'a> = std::sync::MutexGuard<'a, Option<(u64, SenderSidecar)>>;

/// Starts the cast sidecar if it isn't running and returns the lock holding it.
fn ensure_sender(app: &AppHandle) -> Result<SenderSlot<'_>, String> {
    let sidecars = &app.state::<AppState>().inner().sidecars;
    let mut guard = sidecars.sender.lock().unwrap();
    if guard.as_ref().is_some_and(|(_, s)| !s.is_running()) {
        *guard = None;
    }
    if guard.is_some() {
        return Ok(guard);
    }
    let Some(program) = find_sender_program(app) else {
        sender_log_line(app, "no cast sidecar found (no bundled oap-sender, OAP_SENDER or dev script)");
        return Err(CASTING_UNAVAILABLE.into());
    };
    let generation = sidecars.sender_generation.fetch_add(1, Ordering::SeqCst) + 1;
    *sidecars.sender_ready.lock().unwrap() = None;
    sidecars.casting.lock().unwrap().clear();
    set_tray_sending(app, &TRAY_CASTING, false);
    sender_log_line(app, &format!("starting {:?} {:?} ({:?})", program.program, program.args, program.source));
    let (events, exits) = (app.clone(), app.clone());
    let sidecar = SenderSidecar::spawn(
        &program,
        sender_log(app),
        move |event| {
            let sidecars = &events.state::<AppState>().inner().sidecars;
            if event["event"] == "ready" {
                *sidecars.sender_ready.lock().unwrap() = Some(event.clone());
            }
            if let Some((identifier, on)) = sender::casting_change(&event) {
                let mut casting = sidecars.casting.lock().unwrap();
                if on {
                    casting.insert(identifier);
                } else {
                    casting.remove(&identifier);
                }
                set_tray_sending(&events, &TRAY_CASTING, !casting.is_empty());
            }
            let _ = events.emit("sender-event", &event);
        },
        move |exit| {
            if exit.requested {
                return;
            }
            sender_log_line(&exits, &format!("cast sidecar exited unexpectedly: {:?}", exit.code));
            let state = exits.state::<AppState>();
            let mut current = state.sidecars.sender.lock().unwrap();
            if current.as_ref().is_some_and(|(g, _)| *g == generation) {
                *current = None;
                state.sidecars.casting.lock().unwrap().clear();
                set_tray_sending(&exits, &TRAY_CASTING, false);
            }
            drop(current);
            let _ = exits.emit("sender-exit", json!({ "message": CASTING_STOPPED }));
        },
    )
    .map_err(|e| {
        sender_log_line(app, &format!("could not start the cast sidecar: {e}"));
        CASTING_UNAVAILABLE.to_string()
    })?;
    *guard = Some((generation, sidecar));
    Ok(guard)
}

fn send_to_sender(app: &AppHandle, command: &Value) -> Result<(), String> {
    let mut guard = ensure_sender(app)?;
    let result = guard.as_ref().unwrap().1.send(command);
    result.map_err(|e| {
        *guard = None;
        sender_log_line(app, &format!("writing to the cast sidecar failed: {e}"));
        CASTING_STOPPED.to_string()
    })
}

/// Sends one command to the pyatv sidecar, (re)starting it if needed. Replies arrive as "sender-event".
/// If the sidecar dies, "sender-exit" is emitted so the UI can fail outstanding requests and retry.
#[tauri::command]
async fn sender_send(app: AppHandle, command: Value) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || send_to_sender(&app, &command)).await.map_err(|e| e.to_string())?
}

/// Stops casting to one device, or to every device when `identifier` is absent (the tray's Stop sending).
/// The ended cast reports "stopped" through "sender-event". With no sidecar running there is nothing to stop.
pub fn stop_casting(app: &AppHandle, identifier: Option<String>) -> Result<(), String> {
    let running = app.state::<AppState>().sidecars.sender.lock().unwrap().as_ref().is_some_and(|(_, s)| s.is_running());
    if !running {
        return Ok(());
    }
    static NEXT_STOP: AtomicU64 = AtomicU64::new(1);
    let mut command = json!({ "id": format!("stop-{}", NEXT_STOP.fetch_add(1, Ordering::SeqCst)), "cmd": "stop" });
    if let Some(identifier) = identifier {
        command["identifier"] = json!(identifier);
    }
    send_to_sender(app, &command)
}

#[tauri::command]
async fn sender_stop(app: AppHandle, identifier: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || stop_casting(&app, identifier)).await.map_err(|e| e.to_string())?
}

/// Whether casting works in this install.
#[derive(Debug, Clone, Serialize)]
struct SenderHealth {
    available: bool,
    source: Option<sender::SenderSource>,
    /// The casting library's version, from the sidecar's ready line.
    version: Option<String>,
    casting: bool,
    /// A plain sentence when casting isn't available.
    message: Option<String>,
}

/// Starts the cast sidecar if needed and waits for its ready line.
#[tauri::command]
async fn sender_health(app: AppHandle) -> Result<SenderHealth, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source = find_sender_program(&app).map(|p| p.source);
        let unavailable = |message: String| SenderHealth { available: false, source, version: None, casting: false, message: Some(message) };
        if let Err(message) = ensure_sender(&app) {
            return unavailable(message);
        }
        let sidecars = &app.state::<AppState>().inner().sidecars;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(ready) = sidecars.sender_ready.lock().unwrap().clone() {
                let version = ready["pyatv"].as_str().map(str::to_string);
                return SenderHealth { available: true, source, version, casting: sidecars.is_casting(), message: None };
            }
            let running = sidecars.sender.lock().unwrap().as_ref().is_some_and(|(_, s)| s.is_running());
            if !running || std::time::Instant::now() > deadline {
                sender_log_line(&app, "cast sidecar did not announce itself");
                return unavailable(CASTING_UNAVAILABLE.into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// The Go mirror sender: bundled next to the engine (with ffmpeg), then OAP_MIRROR_SENDER, then the
/// dev build from `go build -o bin/open-airplay-sender.exe ./cmd/open-airplay` (ffmpeg from PATH).
fn mirror_sender(app: &AppHandle) -> mirror::SenderCommand {
    let name = exe_name("open-airplay-sender");
    let mut candidates: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        let bin = res.join("engine").join("bin");
        candidates.push((bin.join(&name), vec![bin]));
    }
    if let Some(p) = std::env::var_os("OAP_MIRROR_SENDER") {
        candidates.push((PathBuf::from(p), vec![]));
    }
    let dev = (repo_root().join("bin").join(&name), vec![]);
    let (program, path_prepend) = candidates.into_iter().find(|(p, _)| p.is_file()).unwrap_or(dev);
    let log = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join("mirror.log"))
    });
    mirror::SenderCommand { program, args: vec!["-stdio".into()], path_prepend, log }
}

fn the_mirror(app: &AppHandle) -> Result<mirror::Mirror, String> {
    app.state::<AppState>().mirror.get().cloned().ok_or_else(|| "Mirroring isn't ready yet. Try again.".into())
}

#[tauri::command]
fn mirror_status(app: AppHandle) -> mirror::MirrorSnapshot {
    the_mirror(&app).map(|m| m.snapshot()).unwrap_or_default()
}

/// Mirrors this PC to the TV at `target` (its address from the device list), alongside any TVs it
/// already mirrors to. Progress arrives as "mirror-status" events, one entry per TV.
/// The options are the saved Advanced choices, plus the `display` picked in the sheet (sent only when
/// this PC has more than one).
#[tauri::command]
async fn mirror_start(
    app: AppHandle,
    target: String,
    port: Option<u16>,
    name: String,
    display: Option<i32>,
) -> Result<mirror::MirrorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mirror = the_mirror(&app)?;
        let sender = mirror_sender(&app);
        let caps = mirror.capabilities(&sender).unwrap_or_default();
        let mut options = app.state::<AppState>().prefs.lock().unwrap().mirror.connect_options(caps);
        if let Some(display) = display {
            options["display"] = display.into();
        }
        mirror.start(&sender, &target, port, &name, options)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What the sender can do (today: whether it can send this PC's sound), so the Sound toggle shows
/// only when it would take effect.
#[tauri::command]
async fn mirror_capabilities(app: AppHandle) -> Result<mirror::SenderCapabilities, String> {
    tauri::async_runtime::spawn_blocking(move || the_mirror(&app)?.capabilities(&mirror_sender(&app))).await.map_err(|e| e.to_string())?
}

/// The code shown on a TV, or the password set on it. `target` names the TV; without it, the code
/// goes to the only TV asking.
#[tauri::command]
async fn mirror_code(app: AppHandle, target: Option<String>, code: String) -> Result<mirror::MirrorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || the_mirror(&app)?.submit_code(target.as_deref(), &code)).await.map_err(|e| e.to_string())?
}

/// Removes the TV at `target` from the mirror, or without `target`, stops all.
#[tauri::command]
async fn mirror_stop(app: AppHandle, target: Option<String>) -> Result<mirror::MirrorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || the_mirror(&app)?.stop(target.as_deref())).await.map_err(|e| e.to_string())?
}

/// This PC's displays, for the picker shown when there is more than one.
#[tauri::command]
async fn mirror_displays(app: AppHandle) -> Result<Vec<mirror::Display>, String> {
    tauri::async_runtime::spawn_blocking(move || the_mirror(&app)?.displays(&mirror_sender(&app))).await.map_err(|e| e.to_string())?
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
        let Some(engine) = engine_path_for_doctor(&state.status.snapshot()) else { return };
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
                app.state::<AppState>().panel.allow_own_window();
                std::process::Command::new("explorer.exe").arg("ms-settings:network-status").spawn().map_err(|e| e.to_string())?;
            }
            "firewall_blocks" => {
                let state = app.state::<AppState>();
                let engine = engine_path_for_doctor(&state.status.snapshot()).ok_or("The receiver isn't set up yet.")?;
                let inner = doctor::firewall_fix_script(&engine).replace('\'', "''");
                let launcher = format!(
                    "Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-Command','{inner}'"
                );
                // The UAC prompt and the elevated PowerShell must not close the panel.
                state.panel.holds.fetch_add(1, Ordering::SeqCst);
                let status = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &launcher])
                    .status();
                state.panel.allow_own_window();
                state.panel.holds.fetch_sub(1, Ordering::SeqCst);
                let status = status.map_err(|e| e.to_string())?;
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
            // The panel refreshes its device list (devices on the old network are gone).
            let _ = app.emit("network-changed", ());
            let why = match reason {
                netwatch::Restart::Resumed => "the PC woke up",
                netwatch::Restart::NetworkChanged { .. } => "the network changed",
            };
            // Decided under the lifecycle lock, so a receiver turned off meanwhile stays off.
            let state = app.state::<AppState>();
            let _lifecycle = state.lifecycle.begin();
            if let Some(settings) = settings_to_restart(&state.status.snapshot()) {
                eprintln!("open-airplay: restarting the receiver ({reason:?})");
                if let Err(e) = start_receiver_locked(&app, settings, Some(why)) {
                    update_status(&app, |st| st.last_error = Some(e));
                }
            }
        }
    });
}

/// The work area (screen minus taskbar) the panel opens on: the monitor with the tray icon, else the primary one.
fn panel_work_area(window: &tauri::WebviewWindow, icon: Option<panel::Rect>) -> Option<(panel::Rect, f64)> {
    let monitor = icon
        .and_then(|r| window.monitor_from_point(r.x + r.w / 2.0, r.y + r.h / 2.0).ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())?;
    let area = monitor.work_area();
    let work = panel::Rect { x: area.position.x as f64, y: area.position.y as f64, w: area.size.width as f64, h: area.size.height as f64 };
    Some((work, monitor.scale_factor()))
}

/// Sizes the panel to its content (capped to the screen) and anchors it to the tray icon.
fn place_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let state = app.state::<AppState>();
    let icon = *state.panel.anchor.lock().unwrap();
    let Some((work, scale)) = panel_work_area(&window, icon) else { return };
    let gap = panel::GAP * scale;
    let width = (panel::WIDTH * scale).round();
    let content = state.panel.content_height.lock().unwrap().map(|h| h * scale);
    let height = content.or_else(|| window.inner_size().ok().map(|s| s.height as f64)).unwrap_or(400.0 * scale);
    // Round up: a pixel short and the content scrolls.
    let height = height.ceil().min(panel::max_height(work, gap).floor());
    let (x, y) = panel::position(icon, width, height, work, gap);
    eprintln!("open-airplay: panel at {x:.0},{y:.0} size {width}x{height} (tray icon {icon:?}, work area {work:?})");
    let _ = window.set_size(PhysicalSize::new(width, height));
    let _ = window.set_position(PhysicalPosition::new(x.round(), y.round()));
}

/// Opens the panel above the tray icon (the last clicked position, else wherever the icon is now).
fn show_panel(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.panel.anchor.lock().unwrap().is_none() {
        let rect = app.tray_by_id("main").and_then(|t| t.rect().ok().flatten());
        *state.panel.anchor.lock().unwrap() = rect.map(|r| to_panel_rect(&r, 1.0));
    }
    place_panel(app);
    if let Some(window) = app.get_webview_window("main") {
        state.panel.grace_for(panel::OPEN_GRACE);
        state.panel.visible.store(true, Ordering::SeqCst);
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("panel-shown", ());
    }
}

/// Closes the panel. `dismissed`: it closed itself because something else took the focus.
fn hide_panel(app: &AppHandle, dismissed: bool) {
    let state = app.state::<AppState>();
    state.panel.visible.store(false, Ordering::SeqCst);
    if dismissed {
        *state.panel.last_dismissed.lock().unwrap() = Some(Instant::now());
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

fn to_panel_rect(rect: &tauri::Rect, scale: f64) -> panel::Rect {
    let pos: PhysicalPosition<f64> = rect.position.to_physical(scale);
    let size: PhysicalSize<f64> = rect.size.to_physical(scale);
    panel::Rect { x: pos.x, y: pos.y, w: size.width, h: size.height }
}

/// The UI reports its content height (logical px) whenever it changes; the panel follows it.
#[tauri::command]
fn panel_fit(app: AppHandle, height: f64) {
    if !(height.is_finite() && height > 0.0) {
        return;
    }
    *app.state::<AppState>().panel.content_height.lock().unwrap() = Some(height);
    place_panel(&app);
}

/// Esc on the home page.
#[tauri::command]
fn panel_hide(app: AppHandle) {
    hide_panel(&app, false);
}

/// Closes the panel when another window takes the foreground, like Windows quick settings.
#[cfg(windows)]
fn spawn_foreground_watch(app: AppHandle) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetClassNameW, GetMessageW, GetWindowThreadProcessId, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG,
        WINEVENT_OUTOFCONTEXT,
    };

    static APP: OnceLock<AppHandle> = OnceLock::new();

    unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
        let Some(app) = APP.get() else { return };
        let state = app.state::<AppState>();
        if !state.panel.visible.load(Ordering::SeqCst) {
            return;
        }
        let mut fg = panel::Foreground { none: hwnd.is_null(), ..Default::default() };
        if !hwnd.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            fg.own_process = pid == std::process::id();
            let mut class = [0u16; 256];
            let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            fg.class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        }
        if panel::should_dismiss(&fg, state.panel.in_grace()) {
            hide_panel(app, true);
        }
    }

    let _ = APP.set(app);
    std::thread::spawn(|| unsafe {
        // Out-of-context hooks call back on this thread, which must pump messages.
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, std::ptr::null_mut(), Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
        if hook.is_null() {
            eprintln!("open-airplay: could not watch the foreground window; the panel won't close by itself");
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

#[cfg(not(windows))]
fn spawn_foreground_watch(_app: AppHandle) {}

pub fn run() {
    // A second launch (from Start, say) holds the right to take the foreground; let the running
    // instance it signals use it, so the panel it opens gets the focus.
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
        AllowSetForegroundWindow(ASFW_ANY);
    }
    let app = tauri::Builder::default()
        // A second launch focuses the running app instead of starting a rival receiver on the same ports.
        // It never changes receiving; a stray boot launch while running does nothing.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !launch::is_autostart(&args) {
                show_panel(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
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
            {
                let events = app.handle().clone();
                let mirror = mirror::Mirror::new(
                    move |snapshot| {
                        let _ = events.emit("mirror-status", snapshot);
                        set_tray_sending(&events, &TRAY_MIRRORING, snapshot.is_sending());
                    },
                    || {
                        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
                    },
                );
                let _ = app.state::<AppState>().mirror.set(mirror);
            }
            let launch = launch::plan(&std::env::args().collect::<Vec<_>>(), &app.state::<AppState>().prefs.lock().unwrap());
            let menu = tray_menu(app.handle(), TrayItems::default())?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("bundle icon"))
                .tooltip("open-airplay")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                        let app = tray.app_handle();
                        let state = app.state::<AppState>();
                        *state.panel.anchor.lock().unwrap() = Some(to_panel_rect(&rect, 1.0));
                        let visible = state.panel.visible.load(Ordering::SeqCst);
                        let last = *state.panel.last_dismissed.lock().unwrap();
                        match panel::tray_click(visible, last, Instant::now()) {
                            panel::TrayClick::Open => show_panel(app),
                            panel::TrayClick::Close => hide_panel(app, false),
                            panel::TrayClick::Nothing => {}
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "disconnect" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = disconnect_inner(&app) {
                                eprintln!("open-airplay: {e}");
                            }
                        });
                    }
                    "stop_sending" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            // Everything this PC sends: every mirrored TV and every cast.
                            if let Err(e) = the_mirror(&app).and_then(|m| m.stop(None)) {
                                eprintln!("open-airplay: {e}");
                            }
                            if let Err(e) = stop_casting(&app, None) {
                                eprintln!("open-airplay: {e}");
                            }
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            spawn_network_watcher(app.handle().clone());
            spawn_foreground_watch(app.handle().clone());

            // A boot launch shows the off state in the tray and nothing else. A manual launch opens the
            // panel and, if the switch was left on, starts receiving so the PC shows up without a click.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // The first update always publishes: the tray shows the off look and tooltip.
                update_status(&handle, |_| {});
                if launch.show_panel {
                    show_panel(&handle);
                }
                if !launch.start_receiving {
                    return;
                }
                let saved = handle.state::<AppState>().prefs.lock().unwrap().receiver.clone();
                if let Err(e) = start_receiver_inner(&handle, saved, None) {
                    update_status(&handle, |st| {
                        st.wanted = true;
                        st.last_error = Some(e);
                    });
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![receiver_status, default_settings, get_prefs, set_prefs, show_video_window, doctor_findings, doctor_check, doctor_fix, receiver_start, receiver_stop, sender_send, sender_stop, sender_health, panel_fit, panel_hide, receiver_disconnect, receiver_save_settings, receiver_dismiss_notice, receiver_control, receiver_art, receiver_block_device, receiver_forget_paired, list_speakers, mirror_status, mirror_start, mirror_code, mirror_stop, mirror_displays, set_mirror_options, mirror_capabilities, added_device_check, added_device_save, added_device_remove])
        .build(tauri::generate_context!())
        .expect("error while building open-airplay");

    // Tauri's run loop ends the process without running destructors, so stop and reap the sidecars
    // explicitly on every way out (tray Quit, closing the last window, OS shutdown).
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            if let Some(mirror) = app.state::<AppState>().mirror.get() {
                mirror.shutdown();
            }
            app.state::<AppState>().sidecars.shutdown();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_network_change_restarts_only_a_receiver_that_is_on() {
        let settings = Some(ReceiverSettings { name: "Den".into(), ..Default::default() });
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
    fn cover_art_shows_jpeg_and_png_but_not_the_engine_placeholder() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
        assert_eq!(art_data_url(&jpeg).as_deref(), Some("data:image/jpeg;base64,/9j/4AAQSkZJRg=="));
        // An IHDR chunk: 600x600, then the engine's 1x1 placeholder.
        let png = |w: u32, h: u32| {
            let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R'];
            b.extend(w.to_be_bytes());
            b.extend(h.to_be_bytes());
            b
        };
        assert!(art_data_url(&png(600, 600)).unwrap().starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert_eq!(art_data_url(&png(1, 1)), None);
        assert_eq!(art_data_url(b"no data\n"), None);
        assert_eq!(art_data_url(&[]), None);
    }

    #[test]
    fn a_chosen_speaker_that_is_unplugged_plays_on_the_windows_default() {
        let id = "{0.0.0.00000000}.{597b88b2-8565-49a2-b30e-691eb83dd31a}";
        let chosen = ReceiverSettings { speaker: Some(id.into()), ..Default::default() };
        let plugged = [speakers::Speaker { id: id.into(), name: "Speakers".into() }];
        assert_eq!(engine_settings(chosen.clone(), &plugged).speaker.as_deref(), Some(id));
        assert_eq!(engine_settings(chosen, &[]).speaker, None);
    }

    #[test]
    fn a_restart_scheduled_before_off_stands_down() {
        let lifecycle = Lifecycle::default();
        let ticket = lifecycle.ticket();
        drop(lifecycle.begin()); // the user turns receiving off
        assert_eq!(lifecycle.restart_if_current(ticket, || "restarted"), None);
    }

    #[test]
    fn a_restart_nothing_overtook_runs_once() {
        let lifecycle = Lifecycle::default();
        let ticket = lifecycle.ticket();
        assert_eq!(lifecycle.restart_if_current(ticket, || "restarted"), Some("restarted"));
        assert_eq!(lifecycle.restart_if_current(ticket, || "again"), None, "a second copy of the same restart");
    }

    #[test]
    fn off_pressed_during_a_restart_waits_for_it_and_has_the_last_word() {
        use std::sync::{mpsc, Arc};
        let lifecycle = Arc::new(Lifecycle::default());
        let ticket = lifecycle.ticket();
        let (started, wait) = mpsc::channel();
        let (go, release) = mpsc::channel::<()>();
        let restarting = Arc::clone(&lifecycle);
        let worker = std::thread::spawn(move || {
            restarting.restart_if_current(ticket, || {
                started.send(()).unwrap();
                release.recv().unwrap(); // the restart is mid-way when Off is pressed
            })
        });
        wait.recv().unwrap();
        let off = Arc::clone(&lifecycle);
        let stopper = std::thread::spawn(move || drop(off.begin()));
        go.send(()).unwrap();
        assert!(worker.join().unwrap().is_some());
        stopper.join().unwrap();
        // Off ran after the restart finished, so a later copy of that restart can't undo it.
        assert_eq!(lifecycle.restart_if_current(ticket, || ()), None);
    }

    #[test]
    fn the_tray_offers_disconnect_and_stop_sending_only_when_they_apply() {
        let ids = |disconnect, stop_sending| TrayItems { disconnect, stop_sending }.ids().into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert_eq!(ids(false, false), ["quit"]);
        assert_eq!(ids(false, true), ["stop_sending", "quit"]);
        assert_eq!(ids(true, true), ["disconnect", "stop_sending", "quit"]);
        assert_eq!(TrayItems { disconnect: false, stop_sending: true }.ids()[0].1, "Stop sending");
    }

    #[test]
    fn the_tray_tooltip_says_when_receiving_is_off() {
        assert_eq!(tray_text(&ReceiverStatus::default()), "open-airplay: receiving is off");
    }

    #[test]
    fn saving_settings_never_turns_receiving_on_and_waits_for_a_live_session() {
        let off = ReceiverStatus::default();
        assert_eq!(settings_change(&off), SettingsChange::SaveOnly);
        let problem = ReceiverStatus { wanted: true, last_error: Some("x".into()), ..Default::default() };
        assert_eq!(settings_change(&problem), SettingsChange::SaveOnly);
        let ready = ReceiverStatus { wanted: true, running: true, advertised: true, ..Default::default() };
        assert_eq!(settings_change(&ready), SettingsChange::RestartNow);
        let mirroring = ReceiverStatus { video: Some((886, 1920)), ..ready.clone() };
        assert_eq!(settings_change(&mirroring), SettingsChange::AfterSession);
        let pin = ReceiverStatus { pin: Some("1234".into()), ..ready };
        assert_eq!(settings_change(&pin), SettingsChange::AfterSession);
    }

    #[test]
    fn tray_shows_receiving_during_a_session_and_problem_on_error() {
        assert_eq!(tray_look(Phase::Ready), TrayLook::Normal);
        assert_eq!(tray_look(Phase::Stopped), TrayLook::Normal);
        assert_eq!(tray_look(Phase::Restarting), TrayLook::Normal);
        assert_eq!(tray_look(Phase::Connecting), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Mirroring), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Audio), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Video), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Pin), TrayLook::Receiving);
        assert_eq!(tray_look(Phase::Error), TrayLook::Problem);
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
