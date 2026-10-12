//! Runs a sidecar process, turns its output into events, and restarts it when it dies.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::engine::{parse_line, EngineEvent};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SupervisorEvent {
    Started { pid: u32 },
    Engine { event: EngineEvent },
    Log { line: String },
    Exited { code: Option<i32> },
    SpawnFailed { message: String },
}

#[derive(Debug, Clone)]
pub struct EngineCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Directories prepended to PATH (the engine's DLLs and GStreamer runtime).
    pub path_prepend: Vec<PathBuf>,
    pub env: Vec<(String, String)>,
    pub restart_delay: Duration,
}

const POLL: Duration = Duration::from_millis(25);
/// After the engine exits, how long to wait for its last output lines before reporting the exit.
const DRAIN: Duration = Duration::from_millis(500);

/// Owns the engine process. Threads:
/// - the worker spawns the engine, polls it with `try_wait` (never blocking while holding the
///   child lock, so `stop()` can always reach it to kill it) and restarts it after it exits;
/// - a reader per engine run turns stdout into events (bytes, so bad UTF-8 can't end the loop);
/// - a delivery thread calls `on_event`. `stop()` never waits for it, so a callback that needs the
///   UI thread can't deadlock a UI-thread caller of `stop()`.
pub struct Supervisor {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    /// The running engine's stdin, for control lines (engine patch 0003).
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Supervisor {
    pub fn start(cmd: EngineCommand, on_event: impl Fn(SupervisorEvent) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let (events, inbox) = mpsc::channel::<SupervisorEvent>();
        thread::spawn(move || {
            for event in inbox {
                on_event(event);
            }
        });
        let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
        let (stop2, child2, stdin2) = (stop.clone(), child.clone(), stdin.clone());
        let handle = thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                run_once(&cmd, &stop2, &child2, &stdin2, &events);
                let deadline = Instant::now() + cmd.restart_delay;
                while Instant::now() < deadline && !stop2.load(Ordering::SeqCst) {
                    thread::sleep(POLL);
                }
            }
        });
        Supervisor { stop, child, stdin, handle: Some(handle) }
    }

    /// Sends one control line to the running engine (for example `disconnect`).
    pub fn send_line(&self, line: &str) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        let pipe = stdin.as_mut().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "the engine isn't running"))?;
        pipe.write_all(format!("{line}
").as_bytes())?;
        pipe.flush()
    }

    /// Process id of the engine while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(Child::id)
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One engine run: spawn, stream output, wait for exit (or kill it when stopping).
fn run_once(cmd: &EngineCommand, stop: &AtomicBool, child: &Mutex<Option<Child>>, stdin: &Mutex<Option<ChildStdin>>, events: &Sender<SupervisorEvent>) {
    let mut c = match spawn(cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = events.send(SupervisorEvent::SpawnFailed { message: e.to_string() });
            return;
        }
    };
    crate::process_guard::tie_to_app(&c);
    // The pipe is in place before anyone hears the engine started, so a control line can follow at once.
    *stdin.lock().unwrap() = c.stdin.take();
    let _ = events.send(SupervisorEvent::Started { pid: c.id() });
    let reader = c.stdout.take().map(|out| spawn_reader(out, events.clone()));
    *child.lock().unwrap() = Some(c);

    let code = loop {
        let mut guard = child.lock().unwrap();
        let Some(c) = guard.as_mut() else { break None };
        if stop.load(Ordering::SeqCst) {
            let _ = c.kill();
        }
        match c.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {}
            Err(_) => break None,
        }
        drop(guard);
        thread::sleep(POLL);
    };
    child.lock().unwrap().take();
    stdin.lock().unwrap().take();
    if let Some(done) = reader {
        let _ = done.recv_timeout(DRAIN);
    }
    let _ = events.send(SupervisorEvent::Exited { code });
}

/// Reads engine output line by line until EOF or a read error. Returns a channel that closes when done.
fn spawn_reader(out: impl Read + Send + 'static, events: Sender<SupervisorEvent>) -> mpsc::Receiver<()> {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let _done = done_tx;
        let mut reader = BufReader::new(out);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // A lone "\r" ends a line too: the engine rewrites its music progress line in place.
                    let text = String::from_utf8_lossy(&buf);
                    for line in text.trim_end_matches(['\r', '\n']).split('\r').filter(|l| !l.is_empty()) {
                        let event = match parse_line(line) {
                            Some(event) => SupervisorEvent::Engine { event },
                            None => SupervisorEvent::Log { line: line.to_string() },
                        };
                        if events.send(event).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    done_rx
}

fn spawn(cmd: &EngineCommand) -> std::io::Result<Child> {
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    c.envs(cmd.env.iter().map(|(k, v)| (k, v)));
    if !cmd.path_prepend.is_empty() {
        let mut paths = cmd.path_prepend.clone();
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            c.env("PATH", joined);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A fake engine: prints one UxPlay-style line, then exits.
    fn fake_engine(line: &str) -> EngineCommand {
        #[cfg(windows)]
        let (program, args) = ("cmd".into(), vec!["/C".to_string(), format!("echo {line}")]);
        #[cfg(not(windows))]
        let (program, args) = ("sh".into(), vec!["-c".to_string(), format!("echo '{line}'")]);
        EngineCommand { program, args, path_prepend: vec![], env: vec![], restart_delay: Duration::from_millis(50) }
    }

    fn collect(rx: &mpsc::Receiver<SupervisorEvent>, n: usize) -> Vec<SupervisorEvent> {
        (0..n).map(|_| rx.recv_timeout(Duration::from_secs(10)).expect("event")).collect()
    }

    #[test]
    fn reports_engine_events_and_restarts_after_exit() {
        let (tx, rx) = mpsc::channel();
        let sup = Supervisor::start(fake_engine("Open connections: 1"), move |e| {
            let _ = tx.send(e);
        });
        let events = collect(&rx, 4);
        sup.stop();
        assert!(matches!(events[0], SupervisorEvent::Started { .. }));
        assert_eq!(events[1], SupervisorEvent::Engine { event: EngineEvent::OpenConnections { count: 1 } });
        assert!(matches!(events[2], SupervisorEvent::Exited { .. }));
        assert!(matches!(events[3], SupervisorEvent::Started { .. }), "restarted: {events:?}");
    }

    /// A fake engine written in Python, so it can emit exact bytes and stay alive.
    fn python_engine(script: &str) -> EngineCommand {
        EngineCommand {
            program: if cfg!(windows) { "python".into() } else { "python3".into() },
            args: vec!["-c".into(), script.into()],
            path_prepend: vec![],
            env: vec![],
            restart_delay: Duration::from_secs(60),
        }
    }

    fn stop_within(sup: Supervisor, limit: Duration) {
        let started = std::time::Instant::now();
        sup.stop();
        assert!(started.elapsed() < limit, "stop took {:?}", started.elapsed());
    }

    #[test]
    fn invalid_utf8_is_logged_and_later_lines_still_parse_and_stop_is_prompt() {
        let (tx, rx) = mpsc::channel();
        let script = "import sys,time\nsys.stdout.buffer.write(b'\\xff\\xfe bad\\nOpen connections: 1\\n')\nsys.stdout.flush()\ntime.sleep(30)";
        let sup = Supervisor::start(python_engine(script), move |e| {
            let _ = tx.send(e);
        });
        let events = collect(&rx, 3);
        assert!(matches!(events[0], SupervisorEvent::Started { .. }));
        assert!(matches!(&events[1], SupervisorEvent::Log { line } if line.contains('\u{FFFD}')), "{events:?}");
        assert_eq!(events[2], SupervisorEvent::Engine { event: EngineEvent::OpenConnections { count: 1 } });
        stop_within(sup, Duration::from_secs(2));
    }

    #[test]
    fn progress_lines_ending_in_a_carriage_return_dont_swallow_the_next_line() {
        // UxPlay rewrites its music progress line in place with "\r"; the line after it must still parse.
        let (tx, rx) = mpsc::channel();
        let script = "import sys,time\nsys.stdout.buffer.write(b'audio progress (min:sec):   0:01\\raudio progress (min:sec):   0:02\\ropen-airplay: volume 40\\r\\nOpen connections: 1\\n')\nsys.stdout.flush()\ntime.sleep(30)";
        let sup = Supervisor::start(python_engine(script), move |e| {
            let _ = tx.send(e);
        });
        let events = collect(&rx, 5);
        stop_within(sup, Duration::from_secs(2));
        assert_eq!(
            events[1..],
            [
                SupervisorEvent::Log { line: "audio progress (min:sec):   0:01".into() },
                SupervisorEvent::Log { line: "audio progress (min:sec):   0:02".into() },
                SupervisorEvent::Engine { event: EngineEvent::Volume { percent: 40 } },
                SupervisorEvent::Engine { event: EngineEvent::OpenConnections { count: 1 } },
            ]
        );
    }

    #[test]
    fn stop_is_prompt_when_the_engine_closes_stdout_but_keeps_running() {
        let (tx, rx) = mpsc::channel();
        let sup = Supervisor::start(python_engine("import os,time\nos.close(1)\ntime.sleep(30)"), move |e| {
            let _ = tx.send(e);
        });
        collect(&rx, 1);
        std::thread::sleep(Duration::from_millis(500)); // let the reader hit EOF while the engine lives on
        stop_within(sup, Duration::from_secs(2));
    }

    #[test]
    fn stop_never_waits_for_a_blocked_event_callback() {
        // Models the tray callback that needs the UI thread while the UI thread is the one calling stop().
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (seen_tx, seen_rx) = mpsc::channel();
        let sup = Supervisor::start(python_engine("import time\ntime.sleep(30)"), move |e| {
            let _ = seen_tx.send(e);
            let _ = release_rx.recv(); // blocks until the test releases it
        });
        seen_rx.recv_timeout(Duration::from_secs(10)).expect("started");
        stop_within(sup, Duration::from_secs(2));
        drop(release_tx);
    }

    #[test]
    fn control_lines_reach_the_running_engine() {
        let (tx, rx) = mpsc::channel();
        let echo = "import sys
for line in sys.stdin:
    print('got ' + line.strip(), flush=True)";
        let sup = Supervisor::start(python_engine(echo), move |e| {
            let _ = tx.send(e);
        });
        assert!(matches!(collect(&rx, 1)[0], SupervisorEvent::Started { .. }));
        sup.send_line("disconnect").unwrap();
        assert_eq!(collect(&rx, 1)[0], SupervisorEvent::Log { line: "got disconnect".into() });
        stop_within(sup, Duration::from_secs(2));
    }

    #[test]
    fn a_control_line_without_a_running_engine_is_an_error() {
        let cmd = EngineCommand {
            program: "definitely-not-a-real-engine.exe".into(),
            args: vec![],
            path_prepend: vec![],
            env: vec![],
            restart_delay: Duration::from_secs(60),
        };
        let (tx, rx) = mpsc::channel();
        let sup = Supervisor::start(cmd, move |e| {
            let _ = tx.send(e);
        });
        collect(&rx, 1);
        assert!(sup.send_line("disconnect").is_err());
        sup.stop();
    }

    #[test]
    fn reports_a_missing_engine_without_crashing() {
        let (tx, rx) = mpsc::channel();
        let cmd = EngineCommand {
            program: "definitely-not-a-real-engine.exe".into(),
            args: vec![],
            path_prepend: vec![],
            env: vec![],
            restart_delay: Duration::from_secs(60),
        };
        let sup = Supervisor::start(cmd, move |e| {
            let _ = tx.send(e);
        });
        let events = collect(&rx, 1);
        sup.stop();
        assert!(matches!(events[0], SupervisorEvent::SpawnFailed { .. }));
    }
}
