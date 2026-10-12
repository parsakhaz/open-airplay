//! Mirror options under "Advanced" in the Mirror sheet: the defaults for every mirror. Every field
//! left unset means automatic, and an automatic field is left out of the `options` object sent with
//! `connect` (see docs/control-protocol.md). The display is not here: the sheet picks it each time.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The Advanced choices. `None` means automatic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MirrorOptions {
    pub fps: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub latency_ms: Option<u32>,
    pub video_codec: Option<String>,
    pub hwaccel: Option<String>,
    pub show_cursor: Option<bool>,
    pub audio: Option<bool>,
}

/// What the running sender announced in its `hello` line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SenderCapabilities {
    /// The sender can capture this PC's sound, so the Sound toggle is shown.
    pub audio: bool,
}

impl MirrorOptions {
    /// The `options` object for a `connect`, with automatic values left out. Sound is on unless
    /// turned off, and only sent when the sender can capture it.
    pub fn connect_options(&self, caps: SenderCapabilities) -> Value {
        let mut out = Map::new();
        for (key, value) in [("fps", self.fps), ("bitrate_kbps", self.bitrate_kbps), ("latency_ms", self.latency_ms)] {
            if let Some(value) = value.filter(|v| *v > 0) {
                out.insert(key.into(), value.into());
            }
        }
        for (key, value) in [("video_codec", &self.video_codec), ("hwaccel", &self.hwaccel)] {
            if let Some(value) = value.as_deref().filter(|v| !v.is_empty() && *v != "auto") {
                out.insert(key.into(), value.into());
            }
        }
        if let Some(show) = self.show_cursor {
            out.insert("show_cursor".into(), show.into());
        }
        if caps.audio {
            out.insert("audio".into(), self.audio.unwrap_or(true).into());
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NO_AUDIO: SenderCapabilities = SenderCapabilities { audio: false };
    const AUDIO: SenderCapabilities = SenderCapabilities { audio: true };

    #[test]
    fn untouched_options_send_nothing() {
        assert_eq!(MirrorOptions::default().connect_options(NO_AUDIO), json!({}));
    }

    #[test]
    fn chosen_values_are_sent() {
        let options = MirrorOptions {
            fps: Some(30),
            bitrate_kbps: Some(8000),
            latency_ms: Some(250),
            video_codec: Some("h264".into()),
            hwaccel: Some("none".into()),
            show_cursor: Some(false),
            audio: None,
        };
        assert_eq!(
            options.connect_options(NO_AUDIO),
            json!({"fps": 30, "bitrate_kbps": 8000, "latency_ms": 250, "video_codec": "h264", "hwaccel": "none", "show_cursor": false})
        );
    }

    #[test]
    fn automatic_values_are_left_out() {
        let options = MirrorOptions { fps: Some(0), video_codec: Some("auto".into()), hwaccel: Some("".into()), ..Default::default() };
        assert_eq!(options.connect_options(NO_AUDIO), json!({}));
    }

    #[test]
    fn sound_is_on_by_default_only_once_the_sender_can_capture_it() {
        let mut options = MirrorOptions::default();
        assert_eq!(options.connect_options(NO_AUDIO), json!({}), "never ask for sound the sender would refuse");
        assert_eq!(options.connect_options(AUDIO), json!({"audio": true}));
        options.audio = Some(false);
        assert_eq!(options.connect_options(AUDIO), json!({"audio": false}));
    }

    #[test]
    fn saved_choices_round_trip_and_older_files_load() {
        let options = MirrorOptions { video_codec: Some("h264".into()), ..Default::default() };
        let text = serde_json::to_string(&options).unwrap();
        assert_eq!(serde_json::from_str::<MirrorOptions>(&text).unwrap(), options);
        assert_eq!(serde_json::from_str::<MirrorOptions>("{}").unwrap(), MirrorOptions::default());
    }
}
