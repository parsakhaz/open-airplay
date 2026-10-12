//! Mirroring this PC to an Apple TV: the bundled Go sender (`open-airplay-sender -stdio`) spoken to in
//! JSON lines. Requests go in on stdin with an id; replies come back with that id, and the sender pushes
//! a "state" line whenever its status changes. Pairing keys are the sender's own (Windows Credential
//! Manager, keyed by the TV's device id), so a TV paired with the command-line sender is not asked again.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mirror_targets::MirrorTargets;

/// What the device page shows for mirroring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorPhase {
    #[default]
    Idle,
    Connecting,
    /// The TV shows a code (or has a password) and is waiting for it.
    Code,
    Mirroring,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MirrorStatus {
    pub phase: MirrorPhase,
    /// The TV's address: the join key with the device list.
    pub target: Option<String>,
    /// The TV's name as the device list shows it.
    pub device: Option<String>,
    /// While phase is Code: "pin" (shown on the TV) or "password" (set on the TV).
    pub credential: Option<String>,
    /// One plain sentence: a wrong code, an unreachable TV, a session that ended. Never raw errors.
    pub message: Option<String>,
    /// Unix time in ms when video started flowing.
    pub since_ms: Option<u64>,
}

/// What the app publishes ("mirror-status" event, mirror_status command): one entry per TV this PC
/// is mirroring to, connecting to, or that just failed or ended. Empty when nothing is going on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MirrorSnapshot {
    pub targets: Vec<MirrorStatus>,
}

impl MirrorSnapshot {
    fn of(targets: &MirrorTargets) -> Self {
        MirrorSnapshot { targets: targets.statuses() }
    }

    /// Some TV is connecting, asking for a code, or mirroring: the tray offers Stop sending.
    pub fn is_sending(&self) -> bool {
        self.targets.iter().any(|t| matches!(t.phase, MirrorPhase::Connecting | MirrorPhase::Code | MirrorPhase::Mirroring))
    }
}

/// The parts of the sender's status line the app uses (docs/control-protocol.md).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SenderState {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub streams: Vec<SenderStream>,
    #[serde(default)]
    pub errors: Vec<SenderError>,
}

/// A target's latest failure. `kind` picks the words; `message` is only for the log.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SenderError {
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SenderStream {
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub device_ip: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub credential_kind: String,
}

/// A display this PC can mirror, in the sender's capture order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub index: i32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// What the session asks its owner to do after a state line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    None,
    /// The code was wrong and the TV dropped the attempt: connect again so it shows a new code.
    Reconnect,
}

/// One mirroring session, driven by the sender's lines. Pure: no process, no clock beyond `now_ms`.
#[derive(Debug, Default)]
pub struct MirrorSession {
    status: MirrorStatus,
    /// A connect is in flight: state lines written before its reply may predate it.
    awaiting_reply: bool,
    code_sent: bool,
}

impl MirrorSession {
    pub fn status(&self) -> &MirrorStatus {
        &self.status
    }

    /// Connecting, waiting for a code, or mirroring (not idle after an end, not failed).
    pub fn is_active(&self) -> bool {
        self.status.target.is_some() && !matches!(self.status.phase, MirrorPhase::Error | MirrorPhase::Idle)
    }

    fn name(&self) -> String {
        self.status.device.clone().unwrap_or_else(|| "the TV".into())
    }

    pub fn begin(&mut self, target: &str, name: &str) {
        self.status = MirrorStatus {
            phase: MirrorPhase::Connecting,
            target: Some(target.into()),
            device: Some(name.into()).filter(|n: &String| !n.is_empty()),
            ..Default::default()
        };
        self.awaiting_reply = true;
        self.code_sent = false;
    }

    /// A connect (first try, or again after a wrong code) is being sent.
    pub fn reconnecting(&mut self) {
        self.awaiting_reply = true;
    }

    pub fn connect_replied(&mut self, ok: bool, error_kind: &str) {
        self.awaiting_reply = false;
        if !ok && self.status.target.is_some() {
            let message = words_for(error_kind, &self.name());
            self.fail(message);
        }
    }

    pub fn code_sent(&mut self) {
        self.code_sent = true;
        if self.status.phase == MirrorPhase::Code {
            self.status.phase = MirrorPhase::Connecting;
        }
    }

    pub fn on_state(&mut self, line: &SenderState, now_ms: u64) -> FollowUp {
        let Some(target) = self.status.target.clone() else { return FollowUp::None };
        if !self.is_active() {
            return FollowUp::None;
        }
        if let Some(stream) = line.streams.iter().find(|s| s.device_ip == target) {
            if self.status.device.is_none() && !stream.device.is_empty() {
                self.status.device = Some(stream.device.clone());
            }
            match stream.state.as_str() {
                // Once a code is sent the sender leaves pin_required at once, so a prompt seen after
                // that predates the code. A new prompt only follows a reconnect, which resets code_sent.
                "pin_required" if !self.code_sent => {
                    self.status.phase = MirrorPhase::Code;
                    self.status.credential = Some(if stream.credential_kind.is_empty() { "pin".into() } else { stream.credential_kind.clone() });
                }
                "pin_required" => {}
                "streaming" => {
                    self.status.phase = MirrorPhase::Mirroring;
                    self.status.credential = None;
                    self.status.message = None;
                    self.status.since_ms.get_or_insert(now_ms);
                }
                _ => {
                    if self.status.phase != MirrorPhase::Code {
                        self.status.phase = MirrorPhase::Connecting;
                    }
                }
            }
            return FollowUp::None;
        }
        // The stream is gone. A line written before the connect was handled says nothing about it.
        if self.awaiting_reply {
            return FollowUp::None;
        }
        let kind = line.errors.iter().find(|e| e.target == target).map(|e| e.kind.as_str());
        let name = self.name();
        match kind {
            Some("wrong_code") if self.code_sent => {
                let password = self.status.credential.as_deref() == Some("password");
                self.status.phase = MirrorPhase::Connecting;
                self.status.message = Some(if password {
                    "That password didn't work. Try again.".into()
                } else {
                    format!("That code didn't work. Enter the new code shown on {name}.")
                });
                self.code_sent = false;
                self.awaiting_reply = true;
                FollowUp::Reconnect
            }
            Some("ended") => {
                self.ended(&name);
                FollowUp::None
            }
            Some(kind) => {
                self.fail(words_for(kind, &name));
                FollowUp::None
            }
            None if self.status.phase == MirrorPhase::Mirroring => {
                self.ended(&name);
                FollowUp::None
            }
            None => {
                self.fail(format!("Couldn't mirror to {name}."));
                FollowUp::None
            }
        }
    }

    /// The TV ended the session: keep it listed, idle, with a note.
    fn ended(&mut self, name: &str) {
        self.status = MirrorStatus {
            phase: MirrorPhase::Idle,
            target: self.status.target.take(),
            device: self.status.device.take(),
            message: Some(format!("Mirroring to {name} ended.")),
            ..Default::default()
        };
    }

    /// The user pressed Stop and the sender confirmed.
    pub fn stopped(&mut self) {
        self.status = MirrorStatus::default();
        self.awaiting_reply = false;
        self.code_sent = false;
    }

    /// The sender process ended without being asked to.
    pub fn sender_exited(&mut self) {
        if self.is_active() {
            self.fail("Mirroring stopped unexpectedly. Try again.".into());
        }
    }

    fn fail(&mut self, message: String) {
        self.status.phase = MirrorPhase::Error;
        self.status.credential = None;
        self.status.since_ms = None;
        self.status.message = Some(message);
        self.awaiting_reply = false;
        self.code_sent = false;
    }
}

/// One plain sentence for a sender error kind; the raw text stays in mirror.log.
pub fn words_for(kind: &str, name: &str) -> String {
    match kind {
        "busy" => format!("This PC is already mirroring to {name}."),
        "unreachable" => format!("{name} didn't answer. Check it's on and on the same network."),
        "wrong_code" => "That code didn't work. Try again.".into(),
        "rejected" => format!("{name} didn't accept this PC. Try again."),
        "capture_failed" => "Couldn't capture this PC's screen. Try again, or pick another display.".into(),
        _ => format!("Couldn't mirror to {name}."),
    }
}

/// How to run the sender.
#[derive(Debug, Clone)]
pub struct SenderCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Folders put in front of PATH (where the bundled ffmpeg.exe lives).
    pub path_prepend: Vec<PathBuf>,
    /// The sender's log (stderr), appended to.
    pub log: Option<PathBuf>,
}

#[derive(Clone)]
enum Pending {
    /// A connect to this target: its reply settles whether that target's session started.
    Connect(String),
    /// A status request: its reply is handled like a pushed state line.
    State,
    /// Anything else: the reply goes back to the caller.
    Reply,
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    generation: u64,
}

#[derive(Default)]
struct Inner {
    proc: Option<Proc>,
    generation: u64,
    targets: MirrorTargets,
    next_id: u64,
    pending: HashMap<u64, (Pending, Option<mpsc::Sender<Value>>)>,
    published: Option<MirrorSnapshot>,
}

type OnStatus = Arc<dyn Fn(&MirrorSnapshot) + Send + Sync>;

/// Owns the sender process and one mirroring session per TV. Every status change goes to `on_status`.
#[derive(Clone)]
pub struct Mirror {
    inner: Arc<Mutex<Inner>>,
    on_status: OnStatus,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

impl Mirror {
    pub fn new(on_status: impl Fn(&MirrorSnapshot) + Send + Sync + 'static, now_ms: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Mirror { inner: Arc::default(), on_status: Arc::new(on_status), now_ms: Arc::new(now_ms) }
    }

    pub fn snapshot(&self) -> MirrorSnapshot {
        MirrorSnapshot::of(&self.inner.lock().unwrap().targets)
    }

    /// Starts mirroring to the TV at `target`, alongside any TVs already mirroring.
    pub fn start(&self, sender: &SenderCommand, target: &str, port: Option<u16>, name: &str, display: Option<i32>) -> Result<MirrorSnapshot, String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.targets.is_active(target) {
            // Refused with "Already mirroring to <TV>."
            inner.targets.begin(target, name)?;
        }
        self.ensure_running(&mut inner, sender)?;
        inner.targets.begin(target, name)?;
        let mut request = json!({ "cmd": "connect", "target": target });
        if let Some(port) = port {
            request["port"] = json!(port);
        }
        if let Some(display) = display {
            request["options"] = json!({ "display": display });
        }
        if let Err(e) = self.write(&mut inner, request, Pending::Connect(target.to_string()), None) {
            inner.targets.sender_exited();
            self.publish(&mut inner);
            return Err(e);
        }
        self.publish(&mut inner);
        Ok(MirrorSnapshot::of(&inner.targets))
    }

    /// Sends the code a TV shows (or its password). Without `target`, to the one TV asking.
    pub fn submit_code(&self, target: Option<&str>, code: &str) -> Result<MirrorSnapshot, String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(target) = inner.targets.asking_for_code(target) else {
            return Err("The TV isn't asking for a code right now.".into());
        };
        let code = code.trim();
        if code.is_empty() {
            return Err("Enter the code shown on the TV.".into());
        }
        if let Some(session) = inner.targets.get_mut(&target) {
            session.code_sent();
        }
        self.write(&mut inner, json!({ "cmd": "connect", "target": target, "pin": code }), Pending::Reply, None)?;
        self.publish(&mut inner);
        Ok(MirrorSnapshot::of(&inner.targets))
    }

    /// Removes one TV from the mirror, or every TV (Stop all) when `target` is None. Returns once the
    /// sender has torn it down. A TV that already failed or ended is just taken off the list.
    pub fn stop(&self, target: Option<&str>) -> Result<MirrorSnapshot, String> {
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            let live = match target {
                Some(t) => inner.targets.is_active(t),
                None => inner.targets.any_active(),
            };
            if inner.proc.is_none() || !live {
                Self::forget(&mut inner, target);
                self.publish(&mut inner);
                return Ok(MirrorSnapshot::of(&inner.targets));
            }
            let (tx, rx) = mpsc::channel();
            let request = match target {
                Some(t) => json!({ "cmd": "disconnect", "target": t }),
                None => json!({ "cmd": "disconnect" }),
            };
            self.write(&mut inner, request, Pending::Reply, Some(tx))?;
            rx
        };
        let reply = rx.recv_timeout(REPLY_TIMEOUT);
        let mut inner = self.inner.lock().unwrap();
        if reply.is_err() {
            // No answer: end the process so nothing keeps sending this PC's screen.
            Self::kill(&mut inner);
            inner.targets.sender_exited();
        }
        Self::forget(&mut inner, target);
        self.publish(&mut inner);
        Ok(MirrorSnapshot::of(&inner.targets))
    }

    fn forget(inner: &mut Inner, target: Option<&str>) {
        match target {
            Some(t) => inner.targets.remove(t),
            None => inner.targets.remove_all(),
        }
    }

    /// The displays this PC can mirror.
    pub fn displays(&self, sender: &SenderCommand) -> Result<Vec<Display>, String> {
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            self.ensure_running(&mut inner, sender)?;
            let (tx, rx) = mpsc::channel();
            self.write(&mut inner, json!({ "cmd": "displays" }), Pending::Reply, Some(tx))?;
            rx
        };
        let reply = rx.recv_timeout(REPLY_TIMEOUT).map_err(|_| "Couldn't list this PC's displays.".to_string())?;
        if reply["ok"] != json!(true) {
            return Err("Couldn't list this PC's displays.".into());
        }
        serde_json::from_value(reply["displays"].clone()).map_err(|_| "Couldn't list this PC's displays.".into())
    }

    /// Stops the sender process (app exit). Closing stdin lets it end streams cleanly first.
    pub fn shutdown(&self) {
        let proc = self.inner.lock().unwrap().proc.take();
        if let Some(mut proc) = proc {
            drop(proc.stdin);
            for _ in 0..40 {
                if matches!(proc.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }

    fn kill(inner: &mut Inner) {
        if let Some(mut proc) = inner.proc.take() {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
        inner.pending.clear();
    }

    fn publish(&self, inner: &mut Inner) {
        let snapshot = MirrorSnapshot::of(&inner.targets);
        if inner.published.as_ref() != Some(&snapshot) {
            (self.on_status)(&snapshot);
            inner.published = Some(snapshot);
        }
    }

    fn write(&self, inner: &mut Inner, mut request: Value, kind: Pending, reply: Option<mpsc::Sender<Value>>) -> Result<(), String> {
        inner.next_id += 1;
        let id = inner.next_id;
        request["id"] = json!(id);
        let Some(proc) = inner.proc.as_mut() else { return Err("Mirroring isn't running. Try again.".into()) };
        let line = format!("{request}\n");
        if proc.stdin.write_all(line.as_bytes()).and_then(|_| proc.stdin.flush()).is_err() {
            Self::kill(inner);
            return Err("Mirroring stopped unexpectedly. Try again.".into());
        }
        inner.pending.insert(id, (kind, reply));
        Ok(())
    }

    fn ensure_running(&self, inner: &mut Inner, sender: &SenderCommand) -> Result<(), String> {
        if let Some(proc) = inner.proc.as_mut() {
            if matches!(proc.child.try_wait(), Ok(None)) {
                return Ok(());
            }
            inner.proc = None;
        }
        let mut c = Command::new(&sender.program);
        c.args(&sender.args).stdin(Stdio::piped()).stdout(Stdio::piped());
        let log = sender.log.as_ref().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
        c.stderr(log.map(Stdio::from).unwrap_or_else(Stdio::null));
        if !sender.path_prepend.is_empty() {
            let rest = std::env::var_os("PATH").unwrap_or_default();
            let paths = sender.path_prepend.iter().cloned().chain(std::env::split_paths(&rest));
            if let Ok(path) = std::env::join_paths(paths) {
                c.env("PATH", path);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c.spawn().map_err(|_| "Mirroring isn't installed. Reinstall open-airplay.".to_string())?;
        crate::process_guard::tie_to_app(&child);
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        inner.generation += 1;
        let generation = inner.generation;
        inner.proc = Some(Proc { child, stdin, generation });
        let this = self.clone();
        thread::spawn(move || this.read_lines(stdout, generation));
        Ok(())
    }

    /// Runs on the sender's reader thread: handles lines in the order the sender wrote them.
    fn read_lines(&self, stdout: std::process::ChildStdout, generation: u64) {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            let mut inner = self.inner.lock().unwrap();
            if inner.proc.as_ref().map(|p| p.generation) != Some(generation) {
                return;
            }
            self.handle(&mut inner, message);
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.proc.as_ref().map(|p| p.generation) == Some(generation) {
            Self::kill(&mut inner);
            inner.targets.sender_exited();
            self.publish(&mut inner);
        }
    }

    fn handle(&self, inner: &mut Inner, message: Value) {
        let state_line = match message["type"].as_str() {
            Some("state") => true,
            Some("response") => {
                let id = message["id"].as_u64().unwrap_or(0);
                match inner.pending.remove(&id) {
                    Some((Pending::Connect(target), _)) => {
                        let ok = message["ok"] == json!(true);
                        inner.targets.connect_replied(&target, ok, message["error_kind"].as_str().unwrap_or(""));
                        if ok {
                            // A connect that fails before the next state line would otherwise go unseen.
                            let _ = self.write(inner, json!({ "cmd": "status" }), Pending::State, None);
                        }
                        false
                    }
                    Some((Pending::State, _)) => true,
                    Some((Pending::Reply, reply)) => {
                        if let Some(reply) = reply {
                            let _ = reply.send(message.clone());
                        }
                        false
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if state_line {
            let state: SenderState = serde_json::from_value(message).unwrap_or_default();
            for target in inner.targets.on_state(&state, (self.now_ms)()) {
                if let Some(session) = inner.targets.get_mut(&target) {
                    session.reconnecting();
                }
                let request = json!({ "cmd": "connect", "target": target });
                if self.write(inner, request, Pending::Connect(target), None).is_err() {
                    inner.targets.sender_exited();
                }
            }
        }
        self.publish(inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(state: &str, error: Option<(&str, &str)>, streams: &[(&str, &str, &str)]) -> SenderState {
        SenderState {
            state: state.into(),
            errors: error
                .map(|(target, kind)| vec![SenderError { target: target.into(), kind: kind.into(), message: "detail".into() }])
                .unwrap_or_default(),
            streams: streams
                .iter()
                .map(|(ip, st, kind)| SenderStream { device: "Den TV".into(), device_ip: (*ip).into(), state: (*st).into(), credential_kind: (*kind).into() })
                .collect(),
        }
    }

    const TV: &str = "192.168.1.20";

    fn started() -> MirrorSession {
        let mut s = MirrorSession::default();
        s.begin(TV, "Living Room");
        s.connect_replied(true, "");
        s
    }

    #[test]
    fn a_session_connects_asks_for_the_code_then_mirrors() {
        let mut s = started();
        assert_eq!(s.status().phase, MirrorPhase::Connecting);
        s.on_state(&line("pin_required", None, &[(TV, "pin_required", "pin")]), 1);
        assert_eq!((s.status().phase, s.status().credential.as_deref()), (MirrorPhase::Code, Some("pin")));
        s.code_sent();
        s.on_state(&line("streaming", None, &[(TV, "streaming", "")]), 42);
        let st = s.status();
        assert_eq!((st.phase, st.since_ms, st.device.as_deref()), (MirrorPhase::Mirroring, Some(42), Some("Living Room")));
    }

    #[test]
    fn a_wrong_code_asks_again_with_a_plain_message() {
        let mut s = started();
        s.on_state(&line("pin_required", None, &[(TV, "pin_required", "pin")]), 1);
        s.code_sent();
        let next = s.on_state(&line("idle", Some((TV, "wrong_code")), &[]), 2);
        assert_eq!(next, FollowUp::Reconnect);
        assert_eq!(s.status().phase, MirrorPhase::Connecting);
        assert_eq!(s.status().message.as_deref(), Some("That code didn't work. Enter the new code shown on Living Room."));
        // The new connect's own reply, then the TV's new prompt: the message stays until video starts.
        s.connect_replied(true, "");
        s.on_state(&line("pin_required", None, &[(TV, "pin_required", "pin")]), 3);
        assert_eq!(s.status().phase, MirrorPhase::Code);
        assert!(s.status().message.is_some());
        s.code_sent();
        s.on_state(&line("streaming", None, &[(TV, "streaming", "")]), 4);
        assert_eq!(s.status().message, None);
    }

    #[test]
    fn an_unreachable_tv_is_an_error_in_plain_words() {
        let mut s = started();
        s.on_state(&line("idle", Some((TV, "unreachable")), &[]), 1);
        assert_eq!(s.status().phase, MirrorPhase::Error);
        assert_eq!(s.status().message.as_deref(), Some("Living Room didn't answer. Check it's on and on the same network."));
    }

    #[test]
    fn a_stale_idle_line_before_the_connect_reply_is_ignored() {
        let mut s = MirrorSession::default();
        s.begin(TV, "Living Room");
        s.on_state(&line("idle", Some((TV, "unreachable")), &[]), 1);
        assert_eq!(s.status().phase, MirrorPhase::Connecting);
    }

    #[test]
    fn another_targets_error_is_not_ours() {
        let mut s = started();
        s.on_state(&line("connecting", None, &[(TV, "connecting", "")]), 1);
        s.on_state(&line("idle", Some(("10.0.0.9", "unreachable")), &[(TV, "connecting", "")]), 2);
        assert_eq!(s.status().phase, MirrorPhase::Connecting);
    }

    #[test]
    fn sending_means_some_tv_is_connecting_asking_or_mirroring() {
        let with = |phases: &[MirrorPhase]| MirrorSnapshot { targets: phases.iter().map(|&phase| MirrorStatus { phase, ..Default::default() }).collect() };
        assert!(!with(&[]).is_sending());
        assert!(!with(&[MirrorPhase::Error, MirrorPhase::Idle]).is_sending(), "only notes left");
        assert!(with(&[MirrorPhase::Error, MirrorPhase::Code]).is_sending());
        assert!(with(&[MirrorPhase::Connecting]).is_sending());
        assert!(with(&[MirrorPhase::Idle, MirrorPhase::Mirroring]).is_sending());
    }

    #[test]
    fn a_tv_that_ends_the_session_leaves_a_note() {
        let mut s = started();
        s.on_state(&line("streaming", None, &[(TV, "streaming", "")]), 1);
        s.on_state(&line("idle", None, &[]), 2);
        assert_eq!(s.status().phase, MirrorPhase::Idle);
        assert_eq!(s.status().message.as_deref(), Some("Mirroring to Living Room ended."));
        assert_eq!(s.status().target.as_deref(), Some(TV), "an ended session stays listed with its note");
        assert!(!s.is_active());
    }

    #[test]
    fn stop_returns_to_idle_and_a_crash_is_an_error() {
        let mut s = started();
        s.stopped();
        assert_eq!(*s.status(), MirrorStatus::default());
        let mut s = started();
        s.sender_exited();
        assert_eq!(s.status().phase, MirrorPhase::Error);
        assert_eq!(s.status().message.as_deref(), Some("Mirroring stopped unexpectedly. Try again."));
    }

    #[test]
    fn a_tv_that_stops_the_stream_says_it_ended() {
        let mut s = started();
        s.on_state(&line("streaming", None, &[(TV, "streaming", "")]), 1);
        s.on_state(&line("idle", Some((TV, "ended")), &[]), 2);
        assert_eq!((s.status().phase, s.status().message.as_deref()), (MirrorPhase::Idle, Some("Mirroring to Living Room ended.")));
    }

    #[test]
    fn every_error_kind_has_plain_words() {
        for kind in ["busy", "unreachable", "wrong_code", "rejected", "capture_failed", "ended", "failed", "something_new"] {
            let words = words_for(kind, "Den");
            assert!(words.ends_with('.') && !words.contains('_'), "{kind}: {words}");
        }
    }

    // --- the controller against a fake sender ---

    /// A stand-in for `open-airplay-sender -stdio`: code 1111 is right, anything else is wrong.
    const FAKE_SENDER: &str = r#"
import json, sys
def out(o): print(json.dumps(o), flush=True)
tvs = {}    # target -> "pin_required" | "streaming"
errs = {}   # target -> kind
def status():
    streams = [dict(device="Den TV", device_ip=t, state=st, credential_kind="pin" if st == "pin_required" else "") for t, st in sorted(tvs.items())]
    errors = [dict(target=t, kind=k, message="detail") for t, k in sorted(errs.items())]
    agg = "pin_required" if "pin_required" in tvs.values() else "streaming" if tvs else "idle"
    return dict(ok=True, state=agg, streams=streams, errors=errors)
def state(): out(dict(type="state", **status()))
def reply(rid, **extra): out(dict(type="response", id=rid, **{**status(), **extra}))
out(dict(type="hello", protocol=1))
state()
for raw in sys.stdin:
    req = json.loads(raw); rid = req.get("id"); cmd = req["cmd"]; tv = req.get("target")
    if cmd == "connect" and req.get("pin"):
        reply(rid)
        if req["pin"] == "1111":
            tvs[tv] = "streaming"
        else:
            tvs.pop(tv, None); errs[tv] = "wrong_code"
        state()
    elif cmd == "connect":
        if tv in tvs:
            reply(rid, ok=False, error_kind="busy"); continue
        errs.pop(tv, None); tvs[tv] = "pin_required"
        reply(rid); state()
    elif cmd == "status":
        reply(rid)
    elif cmd == "disconnect":
        if tv: tvs.pop(tv, None)
        else: tvs.clear()
        reply(rid); state()
    elif cmd == "displays":
        reply(rid, displays=[
            dict(index=0, name="\\\\.\\DISPLAY1", x=0, y=0, width=2560, height=1440, primary=True),
            dict(index=1, name="\\\\.\\DISPLAY2", x=2560, y=0, width=1920, height=1080, primary=False)])
    elif cmd == "crash":
        sys.exit(3)
"#;

    fn fake_sender(name: &str) -> SenderCommand {
        let script = std::env::temp_dir().join(format!("oap-fake-mirror-{name}-{}.py", std::process::id()));
        std::fs::write(&script, FAKE_SENDER).unwrap();
        SenderCommand {
            program: PathBuf::from(if cfg!(windows) { "python" } else { "python3" }),
            args: vec!["-u".into(), script.display().to_string()],
            path_prepend: vec![],
            log: None,
        }
    }

    type Statuses = Vec<MirrorStatus>;

    fn watched() -> (Mirror, mpsc::Receiver<Statuses>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Mirror::new(move |s: &MirrorSnapshot| drop(tx.lock().unwrap().send(s.targets.clone())), || 7), rx)
    }

    fn wait_for(rx: &mpsc::Receiver<Statuses>, what: &str, pred: impl Fn(&Statuses) -> bool) -> Statuses {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            if let Ok(s) = rx.recv_timeout(left) {
                if pred(&s) {
                    return s;
                }
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn of<'a>(list: &'a Statuses, target: &str) -> Option<&'a MirrorStatus> {
        list.iter().find(|s| s.target.as_deref() == Some(target))
    }

    fn phase(list: &Statuses, target: &str) -> Option<MirrorPhase> {
        of(list, target).map(|s| s.phase)
    }

    #[test]
    fn pairs_with_a_code_retries_a_wrong_one_and_stops() {
        let sender = fake_sender("pair");
        let (mirror, rx) = watched();
        mirror.start(&sender, "127.0.0.1", Some(7000), "Den TV", Some(1)).unwrap();
        wait_for(&rx, "code prompt", |s| phase(s, "127.0.0.1") == Some(MirrorPhase::Code));
        mirror.submit_code(Some("127.0.0.1"), "0000").unwrap();
        let retry = wait_for(&rx, "second prompt", |s| of(s, "127.0.0.1").is_some_and(|t| t.phase == MirrorPhase::Code && t.message.is_some()));
        assert_eq!(of(&retry, "127.0.0.1").unwrap().message.as_deref(), Some("That code didn't work. Enter the new code shown on Den TV."));
        mirror.submit_code(Some("127.0.0.1"), "1111").unwrap();
        let live = wait_for(&rx, "mirroring", |s| phase(s, "127.0.0.1") == Some(MirrorPhase::Mirroring));
        assert_eq!(of(&live, "127.0.0.1").unwrap().since_ms, Some(7));
        assert_eq!(mirror.start(&sender, "127.0.0.1", None, "Den TV", None).unwrap_err(), "Already mirroring to Den TV.");
        assert_eq!(mirror.stop(None).unwrap(), MirrorSnapshot::default());
        mirror.shutdown();
    }

    #[test]
    fn adds_a_second_tv_to_a_running_mirror_removes_one_and_stops_all() {
        let sender = fake_sender("multi");
        let (mirror, rx) = watched();
        let (a, b) = ("127.0.0.1", "127.0.0.2");
        mirror.start(&sender, a, None, "Living Room", None).unwrap();
        wait_for(&rx, "A's code", |s| phase(s, a) == Some(MirrorPhase::Code));
        mirror.submit_code(Some(a), "1111").unwrap();
        wait_for(&rx, "A live", |s| phase(s, a) == Some(MirrorPhase::Mirroring));

        mirror.start(&sender, b, None, "Bedroom", None).unwrap();
        let both = wait_for(&rx, "B's code", |s| phase(s, b) == Some(MirrorPhase::Code));
        assert_eq!(phase(&both, a), Some(MirrorPhase::Mirroring), "adding B leaves A mirroring");
        assert_eq!(both.iter().map(|s| s.target.clone().unwrap()).collect::<Vec<_>>(), vec![a, b], "in the order added");
        mirror.submit_code(Some(b), "1111").unwrap();
        wait_for(&rx, "both live", |s| phase(s, a) == Some(MirrorPhase::Mirroring) && phase(s, b) == Some(MirrorPhase::Mirroring));

        let left = mirror.stop(Some(a)).unwrap().targets;
        assert_eq!((phase(&left, a), phase(&left, b)), (None, Some(MirrorPhase::Mirroring)));
        // The sender's next lines don't bring A back or touch B.
        mirror.start(&sender, "127.0.0.3", None, "Office", None).unwrap();
        let after = wait_for(&rx, "C's code", |s| phase(s, "127.0.0.3") == Some(MirrorPhase::Code));
        assert_eq!((phase(&after, a), phase(&after, b)), (None, Some(MirrorPhase::Mirroring)));

        assert_eq!(mirror.stop(None).unwrap(), MirrorSnapshot::default());
        mirror.shutdown();
    }

    #[test]
    fn removing_a_tv_that_already_failed_needs_no_sender() {
        let (mirror, rx) = watched();
        let sender = fake_sender("remove-failed");
        mirror.start(&sender, "127.0.0.1", None, "Den TV", None).unwrap();
        wait_for(&rx, "code prompt", |s| phase(s, "127.0.0.1") == Some(MirrorPhase::Code));
        let mut inner = mirror.inner.lock().unwrap();
        mirror.write(&mut inner, json!({ "cmd": "crash" }), Pending::Reply, None).unwrap();
        drop(inner);
        wait_for(&rx, "error", |s| phase(s, "127.0.0.1") == Some(MirrorPhase::Error));
        assert_eq!(mirror.stop(Some("127.0.0.1")).unwrap(), MirrorSnapshot::default());
    }

    #[test]
    fn lists_displays() {
        let (mirror, _rx) = watched();
        let displays = mirror.displays(&fake_sender("displays")).unwrap();
        assert_eq!(displays.len(), 2);
        assert_eq!(displays[1], Display { index: 1, name: r"\\.\DISPLAY2".into(), width: 1920, height: 1080, primary: false });
        mirror.shutdown();
    }

    #[test]
    fn a_sender_crash_mid_session_is_reported() {
        let sender = fake_sender("crash");
        let (mirror, rx) = watched();
        mirror.start(&sender, "127.0.0.1", None, "Den TV", None).unwrap();
        wait_for(&rx, "code prompt", |s| phase(s, "127.0.0.1") == Some(MirrorPhase::Code));
        let mut inner = mirror.inner.lock().unwrap();
        mirror.write(&mut inner, json!({ "cmd": "crash" }), Pending::Reply, None).unwrap();
        drop(inner);
        let crashed = wait_for(&rx, "error", |s| phase(s, "127.0.0.1") == Some(MirrorPhase::Error));
        assert_eq!(of(&crashed, "127.0.0.1").unwrap().message.as_deref(), Some("Mirroring stopped unexpectedly. Try again."));
    }

    #[test]
    fn a_missing_sender_says_to_reinstall() {
        let (mirror, _rx) = watched();
        let missing = SenderCommand { program: "Z:/nope/open-airplay-sender.exe".into(), args: vec![], path_prepend: vec![], log: None };
        assert_eq!(mirror.start(&missing, TV, None, "Den", None).unwrap_err(), "Mirroring isn't installed. Reinstall open-airplay.");
        assert_eq!(mirror.snapshot(), MirrorSnapshot::default());
    }
}
