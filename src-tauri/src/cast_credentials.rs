//! Credentials from pairing a cast device (an Apple TV or speaker that asks for a code), kept per
//! device in a per-user file in the app's config folder (`cast-credentials.json`, not settings.json).
//! The shell adds them to each cast request and takes them out of the sidecar's "paired" reply, so
//! they never reach the panel or a log.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

/// The file's name inside the app config folder.
pub const FILE_NAME: &str = "cast-credentials.json";

/// One device's credentials: protocol name ("AirPlay", "RAOP", or "password") -> credentials.
pub type DeviceCredentials = BTreeMap<String, String>;
/// Every paired device: identifier -> its credentials.
pub type CastCredentials = BTreeMap<String, DeviceCredentials>;

/// Reads the file. A missing or unreadable file means nothing is paired yet.
pub fn load(path: &Path) -> CastCredentials {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Adds a device's new credentials to the file, replacing older ones for the same protocols.
pub fn remember(path: &Path, identifier: &str, credentials: DeviceCredentials) -> std::io::Result<()> {
    let mut all = load(path);
    all.entry(identifier.to_string()).or_default().extend(credentials);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written aside and moved into place, so a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).expect("strings always serialize"))?;
    std::fs::rename(&tmp, path)
}

/// Gives a cast request the device's saved credentials (and drops any the panel sent).
pub fn attach(command: &mut Value, saved: &CastCredentials) {
    if !matches!(command["cmd"].as_str(), Some("play_url" | "stream_file")) {
        return;
    }
    let Some(obj) = command.as_object_mut() else { return };
    obj.remove("credentials");
    let device = obj.get("identifier").and_then(Value::as_str).and_then(|id| saved.get(id));
    if let Some(creds) = device.filter(|c| !c.is_empty()) {
        obj.insert("credentials".into(), serde_json::to_value(creds).expect("strings always serialize"));
    }
}

/// Takes the credentials out of a "paired" reply: `Some((identifier, credentials))` to save. The reply
/// keeps everything else, so the panel still learns the pairing worked.
pub fn take_paired(event: &mut Value) -> Option<(String, DeviceCredentials)> {
    if event["event"] != "paired" {
        return None;
    }
    let creds = event.as_object_mut()?.remove("credentials")?;
    let identifier = event["identifier"].as_str()?.to_string();
    let creds: DeviceCredentials = serde_json::from_value(creds).ok()?;
    (!creds.is_empty()).then_some((identifier, creds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oap-cast-creds-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE_NAME)
    }

    fn creds(pairs: &[(&str, &str)]) -> DeviceCredentials {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn credentials_survive_a_restart_per_device() {
        let path = temp_file("round-trip");
        remember(&path, "tv", creds(&[("AirPlay", "a1")])).unwrap();
        remember(&path, "speaker", creds(&[("RAOP", "r1")])).unwrap();
        remember(&path, "tv", creds(&[("RAOP", "r2")])).unwrap();
        remember(&path, "tv", creds(&[("AirPlay", "a2")])).unwrap();
        // A fresh load is what the app sees after a restart.
        let saved = load(&path);
        assert_eq!(saved["tv"], creds(&[("AirPlay", "a2"), ("RAOP", "r2")]));
        assert_eq!(saved["speaker"], creds(&[("RAOP", "r1")]));
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_or_corrupt_file_means_nothing_is_paired() {
        let path = temp_file("corrupt");
        assert!(load(&path).is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for junk in ["{not json", "[1, 2]", r#"{"tv": "not a map"}"#, ""] {
            std::fs::write(&path, junk).unwrap();
            assert!(load(&path).is_empty(), "{junk}");
        }
        // Pairing again replaces the corrupt file.
        remember(&path, "tv", creds(&[("AirPlay", "a1")])).unwrap();
        assert_eq!(load(&path)["tv"]["AirPlay"], "a1");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn casts_carry_only_their_own_devices_credentials() {
        let mut saved = CastCredentials::new();
        saved.insert("tv".into(), creds(&[("AirPlay", "a1")]));
        let mut cast = json!({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x"});
        attach(&mut cast, &saved);
        assert_eq!(cast["credentials"], json!({"AirPlay": "a1"}));

        let mut other = json!({"id": 2, "cmd": "stream_file", "identifier": "speaker", "path": "C:/a.mp3", "credentials": {"RAOP": "from the panel"}});
        attach(&mut other, &saved);
        assert!(other.get("credentials").is_none());

        let mut scan = json!({"id": 3, "cmd": "scan", "identifier": "tv"});
        attach(&mut scan, &saved);
        assert!(scan.get("credentials").is_none());
    }

    #[test]
    fn a_paired_reply_gives_up_its_credentials() {
        let mut event = json!({"id": 9, "event": "paired", "identifier": "tv", "credentials": {"AirPlay": "a1"}});
        assert_eq!(take_paired(&mut event), Some(("tv".into(), creds(&[("AirPlay", "a1")]))));
        assert_eq!(event, json!({"id": 9, "event": "paired", "identifier": "tv"}));

        let mut other = json!({"id": 1, "event": "playing", "identifier": "tv", "credentials": {"AirPlay": "a1"}});
        assert_eq!(take_paired(&mut other), None);
        assert!(other.get("credentials").is_some());
    }
}
