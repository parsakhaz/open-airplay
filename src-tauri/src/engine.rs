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
    /// With a PIN: a device that entered it once isn't asked again (kept in the app's config folder).
    pub remember_pin: bool,
    /// Let the YouTube app send videos here (UxPlay -hls; other apps' video links can't play).
    pub video_links: bool,
    /// A device that connects while another is connected takes over; otherwise it's turned away.
    pub switch_to_new: bool,
    /// Devices that may never connect.
    pub blocked: Vec<BlockedDevice>,
    /// The Windows output (an endpoint id) the sound plays on; None for the Windows default.
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockedDevice {
    /// The deviceID the phone sends when it connects.
    pub id: String,
    /// Its name when it was blocked, for the Settings list.
    pub name: String,
}

/// In the app's config folder: the receiver's pairing identity, and the devices that entered the PIN.
pub const PAIRING_KEY_FILE: &str = "pairing-key.pem";
pub const PAIRED_DEVICES_FILE: &str = "paired-devices.txt";

/// A Windows audio endpoint id, "{0.0.0.00000000}.{guid}"; anything else never reaches the engine's pipeline.
pub fn is_endpoint_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, '{' | '}' | '.' | '-'))
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
    /// A video link (YouTube) started playing (engine patch 0006).
    VideoLink,
    /// The video link ended: the phone stopped it, it finished, or it was torn down (engine patch 0006).
    VideoLinkStopped,
    /// A video link from an app other than YouTube, which the engine can't play.
    UnsupportedVideoLink,
    /// A second device was turned away because one is already connected.
    Rejected,
    /// A blocked device tried to connect and was refused.
    Blocked { device_id: String },
    Error { message: String },
}

impl ReceiverSettings {
    /// `data_dir`: where the receiver may keep files (the app's config folder), if anywhere.
    pub fn engine_args(&self, data_dir: Option<&Path>) -> Vec<String> {
        // -nh: don't append "@hostname" to the name. No -d: engine patch 0004 prints the lines
        // `parse_line` needs at normal level.
        let mut args = vec!["-n".to_string(), self.name.clone(), "-nh".into()];
        if self.pin_mode {
            args.push("-pin".into());
        }
        if self.h265 {
            args.push("-h265".into());
        }
        if let (true, true, Some(dir)) = (self.pin_mode, self.remember_pin, data_dir) {
            // The key keeps the receiver's identity across restarts, so a remembered device still matches it.
            args.extend(["-key".into(), dir.join(PAIRING_KEY_FILE).display().to_string()]);
            args.extend(["-reg".into(), dir.join(PAIRED_DEVICES_FILE).display().to_string()]);
        }
        if self.video_links {
            args.push("-hls".into());
        }
        if self.switch_to_new {
            args.push("-nohold".into());
        }
        for device in &self.blocked {
            args.extend(["-block".into(), device.id.clone()]);
        }
        if let Some(id) = self.speaker.as_deref().filter(|id| is_endpoint_id(id)) {
            args.extend(["-as".into(), format!("wasapisink device=\"{id}\"")]);
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
    if line == "open-airplay: video link" {
        return Some(EngineEvent::VideoLink);
    }
    if line == "open-airplay: video link stopped" {
        return Some(EngineEvent::VideoLinkStopped);
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
    if line.contains("Unsupported HLS streaming source") || line.contains("Unsupported m3u8 HLS streaming format") {
        return Some(EngineEvent::UnsupportedVideoLink);
    }
    if line.contains("rejecting new connection request from") {
        return Some(EngineEvent::Rejected);
    }
    if let Some(rest) = line.strip_prefix("*** attempt to connect by blocked client (clientID ") {
        let (id, _) = rest.split_once(')')?;
        return Some(EngineEvent::Blocked { device_id: id.trim().into() });
    }
    if line.contains("pairing registration not found") || (line.contains("playlist") && line.contains("not found")) {
        // A device the PC was told to forget (the phone asks for the code again), or something on the
        // network probing the video-link port. Neither is a problem worth a note.
        return None;
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
        ReceiverSettings { name: "Parsa's ZBook".into(), ..Default::default() }
    }

    #[test]
    fn args_advertise_the_exact_name_without_debug_output() {
        assert_eq!(settings().engine_args(None), vec!["-n", "Parsa's ZBook", "-nh"]);
    }

    #[test]
    fn args_add_pin_and_h265_when_enabled() {
        let s = ReceiverSettings { pin_mode: true, h265: true, ..settings() };
        assert_eq!(s.engine_args(None), vec!["-n", "Parsa's ZBook", "-nh", "-pin", "-h265"]);
    }

    #[test]
    fn args_add_video_links_switching_and_blocked_devices() {
        let s = ReceiverSettings {
            video_links: true,
            switch_to_new: true,
            blocked: vec![
                BlockedDevice { id: "1A:2B:3C:4D:5E:6F".into(), name: "Sam's iPhone".into() },
                BlockedDevice { id: "AA:BB:CC:DD:EE:FF".into(), name: "Old iPad".into() },
            ],
            ..settings()
        };
        assert_eq!(
            s.engine_args(None),
            vec!["-n", "Parsa's ZBook", "-nh", "-hls", "-nohold", "-block", "1A:2B:3C:4D:5E:6F", "-block", "AA:BB:CC:DD:EE:FF"]
        );
    }

    #[test]
    fn remembering_pin_devices_keeps_a_key_and_a_register_in_the_data_folder() {
        let dir = std::path::Path::new("C:/oap/config");
        let s = ReceiverSettings { pin_mode: true, remember_pin: true, ..settings() };
        assert_eq!(
            s.engine_args(Some(dir)),
            vec![
                "-n".to_string(),
                "Parsa's ZBook".into(),
                "-nh".into(),
                "-pin".into(),
                "-key".into(),
                dir.join(PAIRING_KEY_FILE).display().to_string(),
                "-reg".into(),
                dir.join(PAIRED_DEVICES_FILE).display().to_string(),
            ]
        );
        // Only with a PIN, and only with somewhere to keep them.
        let no_pin = ReceiverSettings { remember_pin: true, ..settings() };
        assert_eq!(no_pin.engine_args(Some(dir)), vec!["-n", "Parsa's ZBook", "-nh"]);
        assert_eq!(s.engine_args(None), vec!["-n", "Parsa's ZBook", "-nh", "-pin"]);
    }

    #[test]
    fn a_chosen_speaker_plays_through_wasapi() {
        let s = ReceiverSettings { speaker: Some("{0.0.0.00000000}.{8f6c0f4e-1b2a-4c3d-9e8f-0a1b2c3d4e5f}".into()), ..settings() };
        assert_eq!(
            s.engine_args(None),
            vec!["-n", "Parsa's ZBook", "-nh", "-as", "wasapisink device=\"{0.0.0.00000000}.{8f6c0f4e-1b2a-4c3d-9e8f-0a1b2c3d4e5f}\""]
        );
    }

    #[test]
    fn a_speaker_id_that_is_not_a_windows_endpoint_id_is_ignored() {
        for bad in ["", "speakers\" ! fakesink", "{0.0.0.00000000}.{x} ! filesink location=C:/a"] {
            let s = ReceiverSettings { speaker: Some(bad.into()), ..settings() };
            assert_eq!(s.engine_args(None), vec!["-n", "Parsa's ZBook", "-nh"], "{bad}");
        }
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
    fn parses_a_video_link_from_patched_engine() {
        assert_eq!(parse_line("open-airplay: video link"), Some(EngineEvent::VideoLink));
        assert_eq!(parse_line("open-airplay: video link stopped"), Some(EngineEvent::VideoLinkStopped));
    }

    #[test]
    fn parses_an_unsupported_video_link() {
        let line = "*** ERROR Unsupported HLS streaming source: (exit attempt to stream)";
        assert_eq!(parse_line(line), Some(EngineEvent::UnsupportedVideoLink));
        let line = "*** WARNING: Unsupported m3u8 HLS streaming format: clientProcName Vimeo not found in supported list: YouTube;";
        assert_eq!(parse_line(line), Some(EngineEvent::UnsupportedVideoLink));
    }

    #[test]
    fn a_probe_of_the_video_link_port_is_not_an_error() {
        // Seen on a real network with video links on: something sent "HEAD /" to the engine.
        assert_eq!(parse_line("*** ERROR: airplay_video playlist  not found"), None);
    }

    #[test]
    fn parses_a_turned_away_second_device() {
        let line = "*** WARNING: rejecting new connection request from 192.168.1.20";
        assert_eq!(parse_line(line), Some(EngineEvent::Rejected));
    }

    #[test]
    fn parses_a_blocked_device() {
        let line = "*** attempt to connect by blocked client (clientID 1A:2B:3C:4D:5E:6F): DENIED";
        assert_eq!(parse_line(line), Some(EngineEvent::Blocked { device_id: "1A:2B:3C:4D:5E:6F".into() }));
    }

    #[test]
    fn a_forgotten_device_asking_again_is_not_an_error() {
        // With "remember devices" on, a phone the user made the PC forget is asked for the code again.
        assert_eq!(parse_line("*** ERROR: returning client's pairing registration not found: PK=abc="), None);
    }

    #[test]
    fn ignores_noise() {
        assert_eq!(parse_line("raop_rtp_mirror starting mirroring"), None);
        assert_eq!(parse_line(""), None);
    }
}
