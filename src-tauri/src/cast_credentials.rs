//! Credentials from pairing a cast device (an Apple TV or speaker that asks for a code), kept per
//! device in a per-user file in the app's config folder (`cast-credentials.json`, not settings.json).
//! The shell adds them to each cast request and takes them out of the sidecar's "paired" reply, so
//! they never reach the panel or a log.
//!
//! New credentials are kept in memory for this run first, so casting goes on even when the file
//! can't be written. A file that exists but can't be read or parsed is never overwritten: that would
//! lose the pairings in it. Either failure is reported on the "paired" reply (`saved: false`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

/// The file's name inside the app config folder.
pub const FILE_NAME: &str = "cast-credentials.json";

/// One device's credentials: protocol name ("AirPlay", "RAOP", or "password") -> credentials.
pub type DeviceCredentials = BTreeMap<String, String>;
/// Every paired device: identifier -> its credentials.
pub type CastCredentials = BTreeMap<String, DeviceCredentials>;

/// Reads the file. A missing file means nothing is paired yet; one that can't be read or parsed is
/// an error (its pairings are still in it), described without its contents.
pub fn load(path: &Path) -> Result<CastCredentials, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CastCredentials::new()),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    serde_json::from_str(&text).map_err(|e| format!("{} isn't a credentials file ({})", path.display(), parse_problem(&e)))
}

/// Where and what kind of parse error, never the message itself: serde quotes the offending value,
/// which here can be a credential.
fn parse_problem(e: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let kind = match e.classify() {
        Category::Syntax => "not valid JSON",
        Category::Eof => "cut short",
        Category::Data => "wrong shape",
        Category::Io => "unreadable",
    };
    format!("{kind} at line {} column {}", e.line(), e.column())
}

/// The credentials casts use: the saved ones plus this run's pairings (which win). When the file
/// can't be read, this run's pairings still work.
pub fn current(path: Option<&Path>, session: &CastCredentials) -> CastCredentials {
    let mut all = path.and_then(|p| load(p).ok()).unwrap_or_default();
    for (device, creds) in session {
        all.entry(device.clone()).or_default().extend(creds.clone());
    }
    all
}

/// Keeps a device's new credentials: in memory for this run, then in the file. Err when the file
/// couldn't be read (it is left as it is) or written; the credentials still work until the app quits.
pub fn remember(path: Option<&Path>, session: &mut CastCredentials, identifier: &str, credentials: DeviceCredentials) -> Result<(), String> {
    session.entry(identifier.to_string()).or_default().extend(credentials.clone());
    let path = path.ok_or("there's no config folder to save it in")?;
    let mut all = load(path)?;
    all.entry(identifier.to_string()).or_default().extend(credentials);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    // Written aside and moved into place, so a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).expect("strings always serialize"))
        .map_err(|e| format!("couldn't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("couldn't replace {}: {e}", path.display())
    })
}

/// The shell's step for every sidecar event before it reaches the panel. A "paired" reply loses its
/// credentials, which are kept (see `remember`); if they couldn't be saved the reply says
/// `saved: false`, so the panel can tell the user. Returns a line for the log (never credentials).
pub fn on_sidecar_event(path: Option<&Path>, session: &mut CastCredentials, event: &mut Value) -> Option<String> {
    let (identifier, credentials) = take_paired(event)?;
    match remember(path, session, &identifier, credentials) {
        Ok(()) => Some(format!("paired with {identifier}")),
        Err(e) => {
            event["saved"] = Value::Bool(false);
            Some(format!("paired with {identifier}, but the pairing wasn't saved: {e}"))
        }
    }
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
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(FILE_NAME)
    }

    fn creds(pairs: &[(&str, &str)]) -> DeviceCredentials {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn paired(identifier: &str, airplay: &str) -> Value {
        json!({"id": 9, "event": "paired", "identifier": identifier, "credentials": {"AirPlay": airplay}})
    }

    /// TV A paired and saved in an earlier run.
    fn saved_tv_a(name: &str) -> std::path::PathBuf {
        let path = temp_file(name);
        remember(Some(&path), &mut CastCredentials::new(), "tv-a", creds(&[("AirPlay", "a1")])).unwrap();
        path
    }

    #[test]
    fn credentials_survive_a_restart_per_device() {
        let path = temp_file("round-trip");
        let mut session = CastCredentials::new();
        remember(Some(&path), &mut session, "tv", creds(&[("AirPlay", "a1")])).unwrap();
        remember(Some(&path), &mut session, "speaker", creds(&[("RAOP", "r1")])).unwrap();
        remember(Some(&path), &mut session, "tv", creds(&[("RAOP", "r2")])).unwrap();
        remember(Some(&path), &mut session, "tv", creds(&[("AirPlay", "a2")])).unwrap();
        // A fresh load, with no session, is what the app sees after a restart.
        let saved = load(&path).unwrap();
        assert_eq!(saved["tv"], creds(&[("AirPlay", "a2"), ("RAOP", "r2")]));
        assert_eq!(saved["speaker"], creds(&[("RAOP", "r1")]));
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_file_means_nothing_is_paired() {
        let path = temp_file("missing");
        assert_eq!(load(&path), Ok(CastCredentials::new()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_corrupt_file_is_kept_and_the_new_pairing_still_works() {
        let path = temp_file("corrupt");
        for junk in ["{not json", "[1, 2]", r#"{"tv": "not a map"}"#, ""] {
            std::fs::write(&path, junk).unwrap();
            assert!(load(&path).is_err(), "{junk}");
            let mut session = CastCredentials::new();
            let mut event = paired("tv-b", "b1");
            let log = on_sidecar_event(Some(&path), &mut session, &mut event).unwrap();
            assert_eq!(event, json!({"id": 9, "event": "paired", "identifier": "tv-b", "saved": false}), "{junk}");
            assert!(log.contains("wasn't saved") && !log.contains("b1"), "{log}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), junk, "the file is left as it was");
            assert_eq!(current(Some(&path), &session)["tv-b"]["AirPlay"], "b1", "usable until the app quits");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_malformed_file_never_puts_its_contents_in_the_log() {
        const SENTINEL: &str = "SENTINEL-credential-7f3a91";
        let path = temp_file("sentinel");
        // Wrong shapes whose parse errors would quote the value, and broken JSON around it.
        for junk in [
            format!(r#"{{"tv-a": "{SENTINEL}"}}"#),
            format!(r#"{{"tv-a": {{"AirPlay": ["{SENTINEL}"]}}}}"#),
            format!(r#"{{"tv-a": {{"AirPlay": "{SENTINEL}" "#),
            format!(r#"["{SENTINEL}"]"#),
        ] {
            std::fs::write(&path, &junk).unwrap();
            let err = load(&path).unwrap_err();
            assert!(!err.contains(SENTINEL), "load error quotes the file: {err}");
            assert!(err.contains("line 1"), "says where: {err}");
            let mut event = paired("tv-b", "b1");
            let log = on_sidecar_event(Some(&path), &mut CastCredentials::new(), &mut event).unwrap();
            assert!(!log.contains(SENTINEL), "log line quotes the file: {log}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The file is there but can't be read (another program holds it open without sharing reads):
    /// pairing TV B must not replace the file and lose TV A.
    #[cfg(windows)]
    #[test]
    fn a_file_that_cant_be_read_is_not_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_DELETE: u32 = 4;
        let path = saved_tv_a("read-denied");
        let holder = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_DELETE).open(&path).unwrap();
        assert!(load(&path).is_err());
        let mut session = CastCredentials::new();
        let mut event = paired("tv-b", "b1");
        on_sidecar_event(Some(&path), &mut session, &mut event);
        assert_eq!(event["saved"], json!(false));
        assert_eq!(current(Some(&path), &session)["tv-b"]["AirPlay"], "b1");
        drop(holder);
        let saved = load(&path).unwrap();
        assert_eq!(saved["tv-a"]["AirPlay"], "a1", "TV A's pairing is still there");
        assert!(!saved.contains_key("tv-b"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_failed_write_says_so_and_keeps_the_old_file() {
        let path = saved_tv_a("write-fails");
        // Something in the way of the temporary file: the write fails.
        std::fs::create_dir_all(path.with_extension("json.tmp")).unwrap();
        let mut session = CastCredentials::new();
        let mut event = paired("tv-b", "b1");
        let log = on_sidecar_event(Some(&path), &mut session, &mut event).unwrap();
        assert_eq!(event["saved"], json!(false));
        assert!(log.contains("couldn't write"), "{log}");
        assert_eq!(load(&path).unwrap()["tv-a"]["AirPlay"], "a1");
        assert_eq!(current(Some(&path), &session)["tv-b"]["AirPlay"], "b1");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The new file is written but can't be moved into place (the old one is held open without
    /// sharing delete): the old file stays and the temporary one is cleaned up.
    #[cfg(windows)]
    #[test]
    fn a_failed_rename_says_so_and_keeps_the_old_file() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let path = saved_tv_a("rename-fails");
        let holder = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).unwrap();
        let mut session = CastCredentials::new();
        let mut event = paired("tv-b", "b1");
        let log = on_sidecar_event(Some(&path), &mut session, &mut event).unwrap();
        drop(holder);
        assert_eq!(event["saved"], json!(false));
        assert!(log.contains("couldn't replace"), "{log}");
        assert_eq!(load(&path).unwrap()["tv-a"]["AirPlay"], "a1");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_saved_pairing_has_no_saved_flag_and_this_runs_pairings_win() {
        let path = saved_tv_a("saved");
        let mut session = CastCredentials::new();
        let mut event = paired("tv-a", "a2");
        assert_eq!(on_sidecar_event(Some(&path), &mut session, &mut event).as_deref(), Some("paired with tv-a"));
        assert!(event.get("saved").is_none());
        assert_eq!(current(Some(&path), &session)["tv-a"]["AirPlay"], "a2");
        assert_eq!(on_sidecar_event(Some(&path), &mut session, &mut json!({"event": "playing"})), None);
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
