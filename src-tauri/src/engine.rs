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
    /// A phone started playing audio only (music, a podcast), not a mirror's sound (engine patch 0005).
    AudioStart,
    /// The phone stopped its audio stream; the connection may stay open (engine patch 0005).
    AudioStopped,
    /// What's playing; any field may be empty (engine patch 0005).
    Track { title: String, artist: String, album: String },
    /// New cover art is in the `-ca` file (engine patch 0005).
    Art,
    /// The phone's volume slider, 0 to 100 (engine patch 0005).
    Volume { percent: u8 },
    /// The phone's remote-control id, for play/pause/skip over DACP (engine patch 0005).
    Dacp { dacp_id: String, active_remote: String },
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

/// File names, inside the app's data folder, that the engine writes for an audio session.
pub const ART_FILE: &str = "now-playing-art";
pub const DACP_FILE: &str = "dacp-remote";

/// Engine flags for audio sessions: cover art to a file (never plain `-ca`, which draws it in the
/// mirror window and would look like mirroring) and the phone's remote-control id (`-dacp`).
pub fn audio_args(dir: &Path) -> Vec<String> {
    vec!["-ca".into(), dir.join(ART_FILE).display().to_string(), "-dacp".into(), dir.join(DACP_FILE).display().to_string()]
}

pub fn parse_line(line: &str) -> Option<EngineEvent> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("open-airplay: ") {
        if let Some(event) = parse_audio_line(rest) {
            return Some(event);
        }
    }
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

/// The audio-session lines from engine patch 0005 (without the "open-airplay: " prefix).
fn parse_audio_line(rest: &str) -> Option<EngineEvent> {
    match rest {
        "audio start" => return Some(EngineEvent::AudioStart),
        "audio stopped" => return Some(EngineEvent::AudioStopped),
        _ => {}
    }
    let (word, value) = rest.split_once(' ')?;
    match word {
        "track" => {
            // Tab-separated; trailing empty fields may have been trimmed off with the line.
            let mut fields = value.splitn(3, '\t').map(|f| f.trim().to_string());
            let mut next = || fields.next().unwrap_or_default();
            Some(EngineEvent::Track { title: next(), artist: next(), album: next() })
        }
        "art" => Some(EngineEvent::Art),
        "volume" => value.trim().parse().ok().filter(|p| *p <= 100).map(|percent| EngineEvent::Volume { percent }),
        "dacp" => {
            let (dacp_id, active_remote) = value.trim().split_once(' ')?;
            Some(EngineEvent::Dacp { dacp_id: dacp_id.into(), active_remote: active_remote.trim().into() })
        }
        _ => None,
    }
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
    fn parses_the_audio_session_lines_from_patched_engine() {
        assert_eq!(parse_line("open-airplay: audio start"), Some(EngineEvent::AudioStart));
        assert_eq!(parse_line("open-airplay: audio stopped"), Some(EngineEvent::AudioStopped));
        assert_eq!(parse_line("open-airplay: volume 75"), Some(EngineEvent::Volume { percent: 75 }));
        assert_eq!(parse_line("open-airplay: volume 250"), None);
        assert_eq!(parse_line("open-airplay: art 48213"), Some(EngineEvent::Art));
        assert_eq!(
            parse_line("open-airplay: dacp 1A2B3C4D5E6F7788 1986535575"),
            Some(EngineEvent::Dacp { dacp_id: "1A2B3C4D5E6F7788".into(), active_remote: "1986535575".into() })
        );
        assert_eq!(parse_line("open-airplay: dacp 1A2B3C4D5E6F7788"), None);
    }

    #[test]
    fn parses_now_playing_with_empty_fields_and_spaces_kept() {
        assert_eq!(
            parse_line("open-airplay: track Blue in Green\tMiles Davis\tKind of Blue"),
            Some(EngineEvent::Track { title: "Blue in Green".into(), artist: "Miles Davis".into(), album: "Kind of Blue".into() })
        );
        // Radio streams often send only a title.
        assert_eq!(
            parse_line("open-airplay: track Morning Show\t\t"),
            Some(EngineEvent::Track { title: "Morning Show".into(), artist: "".into(), album: "".into() })
        );
    }

    #[test]
    fn audio_args_save_art_and_the_remote_id_to_files_never_the_video_window() {
        let dir = std::path::Path::new("C:/Users/Sam Lee/AppData/Local/open-airplay");
        let args = audio_args(dir);
        assert_eq!(
            args,
            vec![
                "-ca".to_string(),
                dir.join("now-playing-art").display().to_string(),
                "-dacp".to_string(),
                dir.join("dacp-remote").display().to_string(),
            ]
        );
        // Plain "-ca" would draw the art in the mirror window; a following path is what avoids that.
        assert!(!args[1].starts_with('-'));
    }

    #[test]
    fn ignores_noise() {
        assert_eq!(parse_line("raop_rtp_mirror starting mirroring"), None);
        assert_eq!(parse_line(""), None);
    }
}
