//! User preferences, saved as JSON in the app's config folder.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::added::AddedDevice;
use crate::engine::ReceiverSettings;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppPrefs {
    pub receiver: ReceiverSettings,
    /// Launch open-airplay when the user signs in to Windows (quietly, receiving off).
    pub start_at_login: bool,
    /// Set once start at login has been turned on by default, so the user's later "off" sticks.
    pub start_at_login_offered: bool,
    /// The receiving switch as the user last left it; None until they first flip it.
    pub receiving: Option<bool>,
    /// Devices added by IP address or hostname, shown in Send to next to discovered ones.
    pub added_devices: Vec<AddedDevice>,
}

impl AppPrefs {
    /// Reads saved prefs. A missing or unreadable file gives the defaults; `default_name` fills an empty name.
    pub fn load(path: &Path, default_name: &str) -> Self {
        let mut prefs: AppPrefs = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if prefs.receiver.name.trim().is_empty() {
            prefs.receiver.name = default_name.to_string();
        }
        prefs
    }

    /// Turns start at login on once: for new installs, and older ones whose "off" was the old default
    /// rather than a choice. Returns true the one time it does so.
    pub fn offer_start_at_login(&mut self) -> bool {
        if self.start_at_login_offered {
            return false;
        }
        self.start_at_login = true;
        self.start_at_login_offered = true;
        true
    }

    /// Whether a manual launch starts receiving: the remembered switch, on if never chosen.
    pub fn receiving_on(&self) -> bool {
        self.receiving.unwrap_or(true)
    }

    /// Writes atomically (temp file, then rename) so a crash never leaves a half-written file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oap-prefs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[test]
    fn missing_file_gives_defaults_named_after_the_pc() {
        let mut prefs = AppPrefs::load(&temp_path("missing"), "PARSA-DEVBOX");
        assert_eq!(prefs.receiver, ReceiverSettings { name: "PARSA-DEVBOX".into(), pin_mode: false, h265: false });
        assert!(prefs.offer_start_at_login(), "a new install is offered start at login");
        assert!(prefs.start_at_login, "start at login is on by default");
    }

    #[test]
    fn saved_prefs_round_trip_and_the_folder_is_created() {
        let path = temp_path("roundtrip");
        let prefs = AppPrefs {
            receiver: ReceiverSettings { name: "Living Room PC".into(), pin_mode: true, h265: true },
            start_at_login: false,
            start_at_login_offered: true,
            receiving: Some(false),
            added_devices: Vec::new(),
        };
        prefs.save(&path).unwrap();
        assert_eq!(AppPrefs::load(&path, "IGNORED"), prefs);
    }

    #[test]
    fn a_corrupt_file_falls_back_to_defaults() {
        let path = temp_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(AppPrefs::load(&path, "PC").receiver.name, "PC");
    }

    #[test]
    fn older_files_with_missing_fields_still_load() {
        let path = temp_path("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"receiver": {"name": "Den", "pin_mode": true}}"#).unwrap();
        let prefs = AppPrefs::load(&path, "PC");
        assert_eq!(prefs.receiver, ReceiverSettings { name: "Den".into(), pin_mode: true, h265: false });
    }

    #[test]
    fn an_older_install_gets_start_at_login_turned_on_once() {
        let path = temp_path("upgrade");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"receiver": {"name": "Den"}, "start_at_login": false}"#).unwrap();
        let mut prefs = AppPrefs::load(&path, "PC");
        assert!(prefs.offer_start_at_login());
        assert!(prefs.start_at_login);
        prefs.start_at_login = false;
        prefs.save(&path).unwrap();
        let mut next = AppPrefs::load(&path, "PC");
        assert!(!next.offer_start_at_login(), "offered only once");
        assert!(!next.start_at_login, "turning it off survives the next launch");
    }

    #[test]
    fn receiving_is_remembered_and_on_when_never_chosen() {
        let path = temp_path("receiving");
        assert!(AppPrefs::load(&path, "PC").receiving_on());
        AppPrefs { receiving: Some(false), ..AppPrefs::load(&path, "PC") }.save(&path).unwrap();
        assert!(!AppPrefs::load(&path, "PC").receiving_on());
    }

    #[test]
    fn files_from_before_the_full_view_was_removed_still_load() {
        let path = temp_path("full-view");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"receiver": {"name": "Den"}, "open_full_view_on_mirror": true, "start_at_login": true}"#).unwrap();
        let prefs = AppPrefs::load(&path, "PC");
        assert_eq!(prefs.receiver.name, "Den");
        assert!(prefs.start_at_login);
    }

    #[test]
    fn added_devices_are_remembered_in_order() {
        let path = temp_path("added");
        let added = vec![
            AddedDevice { host: "192.168.1.20".into(), port: Some(7100), name: Some("Den".into()), cast_id: Some("AA:BB".into()) },
            AddedDevice { host: "office.local".into(), ..AddedDevice::default() },
        ];
        AppPrefs { added_devices: added.clone(), ..AppPrefs::load(&path, "PC") }.save(&path).unwrap();
        assert_eq!(AppPrefs::load(&path, "PC").added_devices, added);
        assert!(AppPrefs::load(&temp_path("added-none"), "PC").added_devices.is_empty());
    }

    #[test]
    fn a_blank_saved_name_uses_the_pc_name() {
        let path = temp_path("blank");
        AppPrefs { receiver: ReceiverSettings { name: "  ".into(), pin_mode: false, h265: false }, ..AppPrefs::default() }
            .save(&path)
            .unwrap();
        assert_eq!(AppPrefs::load(&path, "PC").receiver.name, "PC");
    }
}
