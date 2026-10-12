//! Mirror options under "Advanced": app-wide defaults in Settings, overridden per device.
//! Every field left unset means automatic, and an automatic field is left out of the
//! `options` object sent with `connect` (see docs/control-protocol.md).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One set of choices. `None` means automatic (or, per device, "use the default").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MirrorOptions {
    pub display: Option<u32>,
    pub fps: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub latency_ms: Option<u32>,
    pub video_codec: Option<String>,
    pub hwaccel: Option<String>,
    pub show_cursor: Option<bool>,
    pub audio: Option<bool>,
}

impl MirrorOptions {
    /// Fields set here win; the rest come from `base`.
    fn over(&self, base: &MirrorOptions) -> MirrorOptions {
        MirrorOptions {
            display: self.display.or(base.display),
            fps: self.fps.or(base.fps),
            bitrate_kbps: self.bitrate_kbps.or(base.bitrate_kbps),
            latency_ms: self.latency_ms.or(base.latency_ms),
            video_codec: self.video_codec.clone().or_else(|| base.video_codec.clone()),
            hwaccel: self.hwaccel.clone().or_else(|| base.hwaccel.clone()),
            show_cursor: self.show_cursor.or(base.show_cursor),
            audio: self.audio.or(base.audio),
        }
    }
}

/// What the running sender announced in its `hello` line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SenderCapabilities {
    /// The sender can capture this PC's sound, so the Sound toggle is shown.
    pub audio: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MirrorPrefs {
    /// Settings > Mirroring: used for every device without its own choice.
    pub defaults: MirrorOptions,
    /// Per device, keyed by the device's AirPlay id (or the address for a device added by IP).
    pub devices: BTreeMap<String, MirrorOptions>,
}

impl MirrorPrefs {
    /// The `options` object for a `connect` to `device`: its overrides over the defaults, with
    /// automatic values left out. Sound is on unless turned off, and only sent when the sender
    /// can capture it; `display` only when this PC has more than one display to choose from.
    pub fn connect_options(&self, device: &str, caps: SenderCapabilities, display_count: usize) -> Value {
        let resolved = match self.devices.get(device) {
            Some(own) => own.over(&self.defaults),
            None => self.defaults.clone(),
        };
        let mut out = Map::new();
        if display_count > 1 {
            if let Some(display) = resolved.display {
                out.insert("display".into(), display.into());
            }
        }
        for (key, value) in [("fps", resolved.fps), ("bitrate_kbps", resolved.bitrate_kbps), ("latency_ms", resolved.latency_ms)] {
            if let Some(value) = value.filter(|v| *v > 0) {
                out.insert(key.into(), value.into());
            }
        }
        for (key, value) in [("video_codec", resolved.video_codec), ("hwaccel", resolved.hwaccel)] {
            if let Some(value) = value.filter(|v| !v.is_empty() && v != "auto") {
                out.insert(key.into(), value.into());
            }
        }
        if let Some(show) = resolved.show_cursor {
            out.insert("show_cursor".into(), show.into());
        }
        if caps.audio {
            out.insert("audio".into(), resolved.audio.unwrap_or(true).into());
        }
        Value::Object(out)
    }

    /// Replaces a device's own choices; all-automatic removes its entry.
    pub fn set_device(&mut self, device: &str, options: MirrorOptions) {
        if options == MirrorOptions::default() {
            self.devices.remove(device);
        } else {
            self.devices.insert(device.to_string(), options);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NO_AUDIO: SenderCapabilities = SenderCapabilities { audio: false };
    const AUDIO: SenderCapabilities = SenderCapabilities { audio: true };

    #[test]
    fn untouched_prefs_send_no_options() {
        assert_eq!(MirrorPrefs::default().connect_options("tv", NO_AUDIO, 1), json!({}));
    }

    #[test]
    fn a_devices_own_choices_win_over_the_defaults_field_by_field() {
        let mut prefs = MirrorPrefs {
            defaults: MirrorOptions { fps: Some(30), hwaccel: Some("none".into()), show_cursor: Some(false), ..Default::default() },
            ..Default::default()
        };
        prefs.set_device("bedroom", MirrorOptions { fps: Some(60), bitrate_kbps: Some(8000), ..Default::default() });
        assert_eq!(
            prefs.connect_options("bedroom", NO_AUDIO, 1),
            json!({"fps": 60, "bitrate_kbps": 8000, "hwaccel": "none", "show_cursor": false})
        );
        assert_eq!(prefs.connect_options("kitchen", NO_AUDIO, 1), json!({"fps": 30, "hwaccel": "none", "show_cursor": false}));
    }

    #[test]
    fn automatic_values_are_left_out() {
        let prefs = MirrorPrefs {
            defaults: MirrorOptions {
                fps: Some(0),
                video_codec: Some("auto".into()),
                hwaccel: Some("".into()),
                latency_ms: Some(250),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(prefs.connect_options("tv", NO_AUDIO, 1), json!({"latency_ms": 250}));
    }

    #[test]
    fn sound_is_on_by_default_only_once_the_sender_can_capture_it() {
        let mut prefs = MirrorPrefs::default();
        assert_eq!(prefs.connect_options("tv", NO_AUDIO, 1), json!({}), "never ask for sound the sender would refuse");
        assert_eq!(prefs.connect_options("tv", AUDIO, 1), json!({"audio": true}));
        prefs.set_device("tv", MirrorOptions { audio: Some(false), ..Default::default() });
        assert_eq!(prefs.connect_options("tv", AUDIO, 1), json!({"audio": false}));
    }

    #[test]
    fn the_display_is_sent_only_with_more_than_one_display() {
        let prefs = MirrorPrefs { defaults: MirrorOptions { display: Some(1), ..Default::default() }, ..Default::default() };
        assert_eq!(prefs.connect_options("tv", NO_AUDIO, 1), json!({}));
        assert_eq!(prefs.connect_options("tv", NO_AUDIO, 2), json!({"display": 1}));
    }

    #[test]
    fn resetting_a_device_to_automatic_forgets_it() {
        let mut prefs = MirrorPrefs::default();
        prefs.set_device("tv", MirrorOptions { fps: Some(60), ..Default::default() });
        prefs.set_device("tv", MirrorOptions::default());
        assert!(prefs.devices.is_empty());
    }

    #[test]
    fn saved_choices_round_trip_and_older_files_load() {
        let mut prefs = MirrorPrefs::default();
        prefs.set_device("AA:BB", MirrorOptions { video_codec: Some("h264".into()), ..Default::default() });
        let text = serde_json::to_string(&prefs).unwrap();
        assert_eq!(serde_json::from_str::<MirrorPrefs>(&text).unwrap(), prefs);
        assert_eq!(serde_json::from_str::<MirrorPrefs>("{}").unwrap(), MirrorPrefs::default());
    }
}
