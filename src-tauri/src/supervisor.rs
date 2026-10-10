//! Runs a sidecar process, turns its output into events, and restarts it when it dies.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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

pub struct Supervisor {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Supervisor {
    pub fn start(cmd: EngineCommand, on_event: impl Fn(SupervisorEvent) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let (stop2, child2) = (stop.clone(), child.clone());
        let handle = thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                match spawn(&cmd) {
                    Ok(mut c) => {
                        on_event(SupervisorEvent::Started { pid: c.id() });
                        let stdout = c.stdout.take();
                        *child2.lock().unwrap() = Some(c);
                        if let Some(out) = stdout {
                            for line in BufReader::new(out).lines().map_while(Result::ok) {
                                match parse_line(&line) {
                                    Some(event) => on_event(SupervisorEvent::Engine { event }),
                                    None => on_event(SupervisorEvent::Log { line }),
                                }
                            }
                        }
                        let code = child2.lock().unwrap().take().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
                        on_event(SupervisorEvent::Exited { code });
                    }
                    Err(e) => on_event(SupervisorEvent::SpawnFailed { message: e.to_string() }),
                }
                // Sleep in small steps so stop() returns promptly.
                let mut waited = Duration::ZERO;
                while waited < cmd.restart_delay && !stop2.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(20));
                    waited += Duration::from_millis(20);
                }
            }
        });
        Supervisor { stop, child, handle: Some(handle) }
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

fn spawn(cmd: &EngineCommand) -> std::io::Result<Child> {
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
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
