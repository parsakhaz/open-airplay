//! The sender seam: a long-lived pyatv sidecar (sender/oap_sender.py) spoken to in JSON lines.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

/// How the sidecar ended. `requested` is true when we stopped it ourselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderExit {
    pub code: Option<i32>,
    pub requested: bool,
}

pub struct SenderSidecar {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    running: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
}

impl SenderSidecar {
    /// Starts `python oap_sender.py`. Every JSON line it prints goes to `on_event`; when the process
    /// ends for any reason (crash, bad install, or `stop()`), `on_exit` is called exactly once, so the
    /// caller can fail outstanding requests instead of leaving them pending.
    pub fn start(
        python: PathBuf,
        script: PathBuf,
        on_event: impl Fn(Value) + Send + 'static,
        on_exit: impl FnOnce(SenderExit) + Send + 'static,
    ) -> std::io::Result<Self> {
        let mut c = Command::new(python);
        c.arg("-u").arg(script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c.spawn()?;
        crate::process_guard::tie_to_app(&child);
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped stdin")));
        let stdout = child.stdout.take().expect("piped stdout");
        let child = Arc::new(Mutex::new(child));
        let running = Arc::new(AtomicBool::new(true));
        let stopping = Arc::new(AtomicBool::new(false));
        let (child2, running2, stopping2) = (child.clone(), running.clone(), stopping.clone());
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(v) = parse_event(&String::from_utf8_lossy(&buf)) {
                            on_event(v);
                        }
                    }
                }
            }
            // Output ended: wait (polling, so stop() can still kill it) for the process to exit.
            let code = loop {
                if let Ok(Some(status)) = child2.lock().unwrap().try_wait() {
                    break status.code();
                }
                thread::sleep(Duration::from_millis(25));
            };
            running2.store(false, Ordering::SeqCst);
            on_exit(SenderExit { code, requested: stopping2.load(Ordering::SeqCst) });
        });
        Ok(SenderSidecar { child, stdin, running, stopping })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn send(&self, command: &Value) -> std::io::Result<()> {
        if !self.is_running() {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "sender sidecar has exited"));
        }
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(encode_command(command).as_bytes())?;
        stdin.flush()
    }

    /// Kills and reaps the sidecar. Returns promptly; `on_exit` reports it with `requested: true`.
    pub fn stop(self) {
        self.kill_and_reap();
    }

    fn kill_and_reap(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for SenderSidecar {
    fn drop(&mut self) {
        if self.is_running() {
            self.kill_and_reap();
        }
    }
}

/// One command per line, no embedded newlines.
pub fn encode_command(command: &Value) -> String {
    format!("{}\n", serde_json::to_string(command).expect("Value always serializes"))
}

/// Only JSON objects carrying an "event" field are protocol events; anything else is ignored.
pub fn parse_event(line: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    v.get("event")?.as_str()?;
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commands_are_single_json_lines() {
        let line = encode_command(&json!({"id": 1, "cmd": "play_url", "url": "http://a/b c.mp4"}));
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let back: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(back["url"], "http://a/b c.mp4");
    }

    use std::sync::mpsc;
    use std::time::Duration;

    /// Writes a throwaway fake sender script and returns (python, script).
    fn fake_sender(name: &str, body: &str) -> (PathBuf, PathBuf) {
        let script = std::env::temp_dir().join(format!("oap-fake-sender-{name}-{}.py", std::process::id()));
        std::fs::write(&script, body).unwrap();
        (PathBuf::from(if cfg!(windows) { "python" } else { "python3" }), script)
    }

    #[test]
    fn reports_when_the_sidecar_dies_after_accepting_a_request() {
        // Announces ready, accepts one request, then crashes without replying.
        let (python, script) = fake_sender(
            "crash",
            "import sys\nprint('{\"event\": \"ready\", \"pyatv\": \"0\"}', flush=True)\nsys.stdin.readline()\nsys.exit(7)\n",
        );
        let (ev_tx, ev_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        let sidecar = SenderSidecar::start(
            python,
            script,
            move |v| {
                let _ = ev_tx.send(v);
            },
            move |exit| {
                let _ = exit_tx.send(exit);
            },
        )
        .unwrap();
        assert_eq!(ev_rx.recv_timeout(Duration::from_secs(10)).unwrap()["event"], "ready");
        sidecar.send(&json!({"id": 17, "cmd": "scan"})).unwrap();
        let exit = exit_rx.recv_timeout(Duration::from_secs(10)).expect("exit reported");
        assert_eq!(exit, SenderExit { code: Some(7), requested: false });
        assert!(!sidecar.is_running());
    }

    #[test]
    fn stopping_the_sidecar_is_reported_as_requested_and_reaps_it() {
        let (python, script) = fake_sender("idle", "import time\ntime.sleep(30)\n");
        let (exit_tx, exit_rx) = mpsc::channel();
        let sidecar = SenderSidecar::start(python, script, |_| {}, move |exit| {
            let _ = exit_tx.send(exit);
        })
        .unwrap();
        let started = std::time::Instant::now();
        sidecar.stop();
        assert!(started.elapsed() < Duration::from_secs(2));
        let exit = exit_rx.recv_timeout(Duration::from_secs(5)).expect("exit reported");
        assert!(exit.requested);
    }

    #[test]
    fn parses_protocol_events_and_ignores_noise() {
        let v = parse_event(r#"{"id": 1, "event": "devices", "devices": []}"#).unwrap();
        assert_eq!(v["event"], "devices");
        assert!(parse_event("Traceback (most recent call last):").is_none());
        assert!(parse_event(r#"{"no_event": true}"#).is_none());
    }
}
