//! User preferences, saved as JSON in the app's config folder.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::ReceiverSettings;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppPrefs {
    pub receiver: ReceiverSettings,
    /// Switch to the full view automatically when a mirroring session starts.
    pub open_full_view_on_mirror: bool,
    /// Launch open-airplay when the user signs in to Windows.
    pub start_at_login: bool,
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
        let prefs = AppPrefs::load(&temp_path("missing"), "PARSA-DEVBOX");
        assert_eq!(prefs.receiver, ReceiverSettings { name: "PARSA-DEVBOX".into(), pin_mode: false, h265: false });
        assert!(!prefs.open_full_view_on_mirror);
        assert!(!prefs.start_at_login);
    }

    #[test]
    fn saved_prefs_round_trip_and_the_folder_is_created() {
        let path = temp_path("roundtrip");
        let prefs = AppPrefs {
            receiver: ReceiverSettings { name: "Living Room PC".into(), pin_mode: true, h265: true },
            open_full_view_on_mirror: true,
            start_at_login: true,
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
        assert!(!prefs.start_at_login);
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
