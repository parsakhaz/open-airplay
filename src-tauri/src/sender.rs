//! The sender seam: a long-lived pyatv sidecar (sender/oap_sender.py) spoken to in JSON lines.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;

pub struct SenderSidecar {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

impl SenderSidecar {
    /// Starts `python oap_sender.py`; every JSON line it prints is passed to `on_event`.
    pub fn start(python: PathBuf, script: PathBuf, on_event: impl Fn(Value) + Send + 'static) -> std::io::Result<Self> {
        let mut c = Command::new(python);
        c.arg("-u").arg(script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c.spawn()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped stdin")));
        let stdout = child.stdout.take().expect("piped stdout");
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(v) = parse_event(&line) {
                    on_event(v);
                }
            }
        });
        Ok(SenderSidecar { child, stdin })
    }

    pub fn send(&self, command: &Value) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(encode_command(command).as_bytes())?;
        stdin.flush()
    }
}

impl Drop for SenderSidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
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

    #[test]
    fn parses_protocol_events_and_ignores_noise() {
        let v = parse_event(r#"{"id": 1, "event": "devices", "devices": []}"#).unwrap();
        assert_eq!(v["event"], "devices");
        assert!(parse_event("Traceback (most recent call last):").is_none());
        assert!(parse_event(r#"{"no_event": true}"#).is_none());
    }
}
