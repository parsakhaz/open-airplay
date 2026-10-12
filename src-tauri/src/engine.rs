//! The receiver engine seam: how the shell talks to the UxPlay sidecar.
//!
//! Two directions:
//! - `ReceiverSettings::engine_args` turns the shell's settings into UxPlay CLI flags.
//! - `parse_line` turns one line of UxPlay output into an `EngineEvent`.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceiverSettings {
    /// Name shown in the iPhone's Screen Mirroring list.
    pub name: String,
    /// Opt-in: the iPhone must enter a 4-digit code shown on the PC.
    pub pin_mode: bool,
    /// Prefer H.265 (some iOS 27 clients need it).
    pub h265: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    Advertised,
    ConnectionRequest { name: String, model: String, device_id: String },
    OpenConnections { count: u32 },
    ClientLost,
    Pin { code: String },
    /// First frame / new resolution of the mirrored video (engine patch 0002).
    Video { width: u32, height: u32 },
    /// The mirrored video stopped; the connection may still be open (engine patch 0003).
    VideoStopped,
    /// The engine ended the session after a `disconnect` control line (engine patch 0003).
    Disconnected,
    Error { message: String },
}

impl ReceiverSettings {
    pub fn engine_args(&self) -> Vec<String> {
        // -nh: don't append "@hostname" to the name. No -d: engine patch 0004 prints the lines
        // `parse_line` needs at normal level.
        let mut args = vec!["-n".to_string(), self.name.clone(), "-nh".into()];
        if self.pin_mode {
            args.push("-pin".into());
        }
        if self.h265 {
            args.push("-h265".into());
        }
        args
    }

    /// Environment for the engine's own mirror window (engine patch 0002).
    pub fn engine_env(&self, icon: Option<&Path>) -> Vec<(String, String)> {
        let mut env = vec![("OAP_WINDOW_TITLE".to_string(), "open-airplay".to_string())];
        if let Some(icon) = icon {
            env.push(("OAP_WINDOW_ICON".into(), icon.display().to_string()));
        }
        env
    }
}

pub fn parse_line(line: &str) -> Option<EngineEvent> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("connection request from ") {
        // "<name> (<model>) with deviceID = <id>"; the name itself may contain parentheses.
        let (head, device_id) = rest.rsplit_once(" with deviceID = ")?;
        let (name, model) = head.strip_suffix(')')?.rsplit_once(" (")?;
        return Some(EngineEvent::ConnectionRequest {
            name: name.into(),
            model: model.into(),
            device_id: device_id.trim().into(),
        });
    }
    if let Some(n) = line.strip_prefix("Open connections: ") {
        return n.trim().parse().ok().map(|count| EngineEvent::OpenConnections { count });
    }
    if line == "open-airplay: video stopped" {
        return Some(EngineEvent::VideoStopped);
    }
    if line == "open-airplay: disconnected" {
        return Some(EngineEvent::Disconnected);
    }
    if let Some(size) = line.strip_prefix("open-airplay: video ") {
        let (w, h) = size.split_once('x')?;
        return Some(EngineEvent::Video { width: w.trim().parse().ok()?, height: h.trim().parse().ok()? });
    }
    if let Some(code) = line.strip_prefix("open-airplay: pin = ") {
        return Some(EngineEvent::Pin { code: code.into() });
    }
    if line.starts_with("register_dnssd: advertised AirPlay service") {
        return Some(EngineEvent::Advertised);
    }
    if line.contains("lost connection with client") {
        return Some(EngineEvent::ClientLost);
    }
    if let Some(message) = line.strip_prefix("*** ERROR: ") {
        return Some(EngineEvent::Error { message: message.into() });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ReceiverSettings {
        ReceiverSettings { name: "Parsa's ZBook".into(), pin_mode: false, h265: false }
    }

    #[test]
    fn args_advertise_the_exact_name_without_debug_output() {
        assert_eq!(settings().engine_args(), vec!["-n", "Parsa's ZBook", "-nh"]);
    }

    #[test]
    fn args_add_pin_and_h265_when_enabled() {
        let s = ReceiverSettings { pin_mode: true, h265: true, ..settings() };
        assert_eq!(s.engine_args(), vec!["-n", "Parsa's ZBook", "-nh", "-pin", "-h265"]);
    }

    #[test]
    fn env_names_the_mirror_window_after_the_app_and_passes_the_icon() {
        let env = settings().engine_env(Some(std::path::Path::new("C:/oap/icons/icon.ico")));
        assert_eq!(
            env,
            vec![
                ("OAP_WINDOW_TITLE".to_string(), "open-airplay".to_string()),
                ("OAP_WINDOW_ICON".to_string(), "C:/oap/icons/icon.ico".to_string()),
            ]
        );
        assert_eq!(settings().engine_env(None), vec![("OAP_WINDOW_TITLE".to_string(), "open-airplay".to_string())]);
    }

    #[test]
    fn parses_connection_request() {
        let line = "connection request from Parsa's iPhone (iPhone16,1) with deviceID = 1A:2B:3C:4D:5E:6F";
        assert_eq!(
            parse_line(line),
            Some(EngineEvent::ConnectionRequest {
                name: "Parsa's iPhone".into(),
                model: "iPhone16,1".into(),
                device_id: "1A:2B:3C:4D:5E:6F".into(),
            })
        );
    }

    #[test]
    fn parses_open_connection_count() {
        assert_eq!(parse_line("Open connections: 2"), Some(EngineEvent::OpenConnections { count: 2 }));
        assert_eq!(parse_line("Open connections: 0"), Some(EngineEvent::OpenConnections { count: 0 }));
    }

    #[test]
    fn parses_advertisement() {
        let line = "register_dnssd: advertised AirPlay service with \"Features\" code = 0x5A7FFEE6";
        assert_eq!(parse_line(line), Some(EngineEvent::Advertised));
    }

    #[test]
    fn parses_lost_client() {
        assert_eq!(parse_line("*** ERROR lost connection with client (network problem?)"), Some(EngineEvent::ClientLost));
    }

    #[test]
    fn parses_errors() {
        assert_eq!(
            parse_line("*** ERROR: failed to open DACP export file \"x\""),
            Some(EngineEvent::Error { message: "failed to open DACP export file \"x\"".into() })
        );
    }

    #[test]
    fn parses_video_size_from_patched_engine() {
        assert_eq!(parse_line("open-airplay: video 886x1920"), Some(EngineEvent::Video { width: 886, height: 1920 }));
        assert_eq!(parse_line("open-airplay: video 0xbogus"), None);
    }

    #[test]
    fn parses_video_stopped_and_the_disconnect_ack_from_patched_engine() {
        assert_eq!(parse_line("open-airplay: video stopped"), Some(EngineEvent::VideoStopped));
        assert_eq!(parse_line("open-airplay: disconnected"), Some(EngineEvent::Disconnected));
    }

    #[test]
    fn parses_pin_from_patched_engine() {
        assert_eq!(parse_line("open-airplay: pin = 4821"), Some(EngineEvent::Pin { code: "4821".into() }));
    }

    /// Output of the patched engine run without -d (as the shell runs it), captured from a real
    /// loopback mirror session with the repo's sender; names and addresses replaced.
    #[test]
    fn sees_every_event_in_a_real_session_without_debug_output() {
        let events: Vec<_> = include_str!("testdata/engine-session-no-debug.txt").lines().filter_map(parse_line).collect();
        let open = |count| EngineEvent::OpenConnections { count };
        let error = |message: &str| EngineEvent::Error { message: message.into() };
        assert_eq!(
            events,
            vec![
                EngineEvent::Advertised,
                open(1),
                open(2),
                open(1),
                EngineEvent::ConnectionRequest {
                    name: "Test Sender".into(),
                    model: "Linux".into(),
                    device_id: "02:00:00:00:00:02".into(),
                },
                open(2),
                open(1),
                error("  3 seconds since last client feedback request (expected every two seconds); client may be offline"),
                EngineEvent::Video { width: 1920, height: 1080 },
                open(2),
                open(1),
                open(2),
                open(1),
                error("httpd: recv error 10054 on socket 7744: An existing connection was forcibly closed by the remote host."),
                open(0),
                error("raop_rtp_mirror error  in header recv: 10054 An existing connection was forcibly closed by the remote host."),
                EngineEvent::ClientLost,
            ]
        );
    }

    #[test]
    fn ignores_noise() {
        assert_eq!(parse_line("raop_rtp_mirror starting mirroring"), None);
        assert_eq!(parse_line(""), None);
    }
}
