//! Credentials from pairing a cast device (an Apple TV or speaker that asks for a code), kept per
//! device in Windows Credential Manager (see `Keyring`), the store the mirror sender uses too. Off
//! Windows they stay in the plaintext file (see `FileStore`).
//! The shell adds them to each cast request and takes them out of the sidecar's "paired" reply, so
//! they never reach the panel or a log.
//!
//! New credentials are kept in memory for this run first, so casting goes on even when the store
//! can't be written. A device whose saved entry can't be read is never overwritten: that would lose
//! the pairings in it. Either failure is reported on the "paired" reply (`saved: false`).
//!
//! Earlier versions kept them in a plaintext file, `cast-credentials.json`; on Windows `migrate`
//! moves its pairings into Credential Manager once, then deletes it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The old plaintext file's name inside the app config folder.
pub const FILE_NAME: &str = "cast-credentials.json";

/// Credential Manager entries are named `<SERVICE>:<device identifier>` (the uninstaller deletes
/// them by this prefix). Not the mirror sender's "open-airplay": the two never share entries.
pub const SERVICE: &str = "open-airplay-cast";

/// One device's credentials: protocol name ("AirPlay", "RAOP", or "password") -> credentials.
pub type DeviceCredentials = BTreeMap<String, String>;
/// Every paired device: identifier -> its credentials.
pub type CastCredentials = BTreeMap<String, DeviceCredentials>;

/// Where saved pairings live, one entry per device. Errors describe what failed, never the contents.
pub trait Store: Send + Sync {
    /// The device's saved credentials; Ok(None) when it was never paired.
    fn get(&self, identifier: &str) -> Result<Option<DeviceCredentials>, String>;
    /// Replaces the device's saved credentials.
    fn set(&self, identifier: &str, credentials: &DeviceCredentials) -> Result<(), String>;
}

impl<T: Store + ?Sized> Store for &T {
    fn get(&self, identifier: &str) -> Result<Option<DeviceCredentials>, String> {
        (**self).get(identifier)
    }
    fn set(&self, identifier: &str, credentials: &DeviceCredentials) -> Result<(), String> {
        (**self).set(identifier, credentials)
    }
}

/// A store, with the old file's pairings under its own while that file is still there (a move into
/// the store that hasn't finished): a pairing the move couldn't take yet still works. The store's
/// pairings win; writes go to the store only.
pub struct LegacyFallback<S> {
    store: S,
    legacy: PathBuf,
}

impl<S: Store> LegacyFallback<S> {
    pub fn new(store: S, legacy: PathBuf) -> Self {
        LegacyFallback { store, legacy }
    }
}

impl<S: Store> Store for LegacyFallback<S> {
    fn get(&self, identifier: &str) -> Result<Option<DeviceCredentials>, String> {
        let saved = self.store.get(identifier)?;
        // An unreadable file adds nothing; migrate reports it.
        let Some(mut creds) = load_file(&self.legacy).ok().and_then(|mut all| all.remove(identifier)) else {
            return Ok(saved);
        };
        creds.extend(saved.unwrap_or_default());
        Ok(Some(creds))
    }

    fn set(&self, identifier: &str, credentials: &DeviceCredentials) -> Result<(), String> {
        self.store.set(identifier, credentials)
    }
}

/// The plaintext `cast-credentials.json`: where pairings live off Windows, where keyring has no
/// persistent store this app enables (its fallback keeps nothing).
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        FileStore { path }
    }
}

impl Store for FileStore {
    fn get(&self, identifier: &str) -> Result<Option<DeviceCredentials>, String> {
        Ok(load_file(&self.path)?.remove(identifier))
    }

    /// A file that can't be read or parsed is never overwritten: that would lose the pairings in it.
    fn set(&self, identifier: &str, credentials: &DeviceCredentials) -> Result<(), String> {
        let path = &self.path;
        let mut all = load_file(path)?;
        all.insert(identifier.to_string(), credentials.clone());
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
}

/// The per-user Windows Credential Manager (a generic credential per device).
#[cfg(windows)]
pub struct Keyring {
    service: String,
}

#[cfg(windows)]
impl Keyring {
    pub fn new(service: &str) -> Self {
        Keyring { service: service.to_string() }
    }

    fn entry(&self, identifier: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new_with_target(&format!("{}:{identifier}", self.service), &self.service, identifier).map_err(describe)
    }

    /// Removes the device's entry, if any.
    pub fn delete(&self, identifier: &str) -> Result<(), String> {
        match self.entry(identifier)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(describe(e)),
        }
    }
}

#[cfg(windows)]
impl Store for Keyring {
    fn get(&self, identifier: &str) -> Result<Option<DeviceCredentials>, String> {
        let text = match self.entry(identifier)?.get_password() {
            Ok(text) => text,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(describe(e)),
        };
        serde_json::from_str(&text).map(Some).map_err(|e| format!("the saved entry for {identifier} isn't readable ({})", parse_problem(&e)))
    }

    fn set(&self, identifier: &str, credentials: &DeviceCredentials) -> Result<(), String> {
        let text = serde_json::to_string(credentials).expect("strings always serialize");
        self.entry(identifier)?.set_password(&text).map_err(describe)
    }
}

/// A keyring error in words. keyring's own messages name the failure and never the secret.
#[cfg(windows)]
fn describe(e: keyring::Error) -> String {
    format!("Credential Manager: {e}")
}

/// Reads the old file. A missing file means nothing to move; one that can't be read or parsed is
/// an error (its pairings are still in it), described without its contents.
pub fn load_file(path: &Path) -> Result<CastCredentials, String> {
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

/// Moves the old file's pairings into the store, then deletes the file. Pairings already in the
/// store win over the file's. Each move is read back: if the file can't be read, or any device
/// can't be moved or isn't there afterwards (a store that keeps nothing), the file is kept as it is
/// and the move is tried again at the next launch. Returns a line for the log (never credentials),
/// or None when there was no file.
pub fn migrate(path: &Path, store: &dyn Store) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let all = match load_file(path) {
        Ok(all) => all,
        Err(e) => return Some(format!("kept {FILE_NAME}, its pairings weren't moved to Credential Manager: {e}")),
    };
    for (identifier, credentials) in &all {
        let moved = store.get(identifier).and_then(|saved| {
            let mut merged = credentials.clone();
            merged.extend(saved.unwrap_or_default());
            store.set(identifier, &merged)?;
            match store.get(identifier)? {
                Some(kept) if kept == merged => Ok(()),
                _ => Err("the store didn't keep it".to_string()),
            }
        });
        if let Err(e) = moved {
            return Some(format!("kept {FILE_NAME}, couldn't move the pairing with {identifier} to Credential Manager: {e}"));
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => Some(format!("moved {} pairing(s) from {FILE_NAME} to Credential Manager", all.len())),
        Err(e) => Some(format!("moved the pairings to Credential Manager, but couldn't delete {FILE_NAME}: {e}")),
    }
}

/// The credentials a cast to `identifier` uses: the saved ones plus this run's pairing (which
/// wins). When the store can't be read, this run's pairing still works.
pub fn credentials_for(store: &dyn Store, session: &CastCredentials, identifier: &str) -> DeviceCredentials {
    let mut creds = store.get(identifier).ok().flatten().unwrap_or_default();
    creds.extend(session.get(identifier).cloned().unwrap_or_default());
    creds
}

/// Keeps a device's new credentials: in memory for this run, then in the store. Err when its saved
/// entry couldn't be read (it is left as it is) or written; the credentials still work until the
/// app quits.
pub fn remember(store: &dyn Store, session: &mut CastCredentials, identifier: &str, credentials: DeviceCredentials) -> Result<(), String> {
    session.entry(identifier.to_string()).or_default().extend(credentials.clone());
    let mut saved = store.get(identifier)?.unwrap_or_default();
    saved.extend(credentials);
    store.set(identifier, &saved)
}

/// The shell's step for every sidecar event before it reaches the panel. A "paired" reply loses its
/// credentials, which are kept (see `remember`); if they couldn't be saved the reply says
/// `saved: false`, so the panel can tell the user. Returns a line for the log (never credentials).
pub fn on_sidecar_event(store: &dyn Store, session: &mut CastCredentials, event: &mut Value) -> Option<String> {
    let (identifier, credentials) = take_paired(event)?;
    match remember(store, session, &identifier, credentials) {
        Ok(()) => Some(format!("paired with {identifier}")),
        Err(e) => {
            event["saved"] = Value::Bool(false);
            Some(format!("paired with {identifier}, but the pairing wasn't saved: {e}"))
        }
    }
}

/// Gives a cast request its device's credentials from `lookup` (and drops any the panel sent).
pub fn attach(command: &mut Value, lookup: impl FnOnce(&str) -> DeviceCredentials) {
    if !matches!(command["cmd"].as_str(), Some("play_url" | "stream_file")) {
        return;
    }
    let Some(obj) = command.as_object_mut() else { return };
    obj.remove("credentials");
    let creds = obj.get("identifier").and_then(Value::as_str).map(lookup).unwrap_or_default();
    if !creds.is_empty() {
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
    use std::sync::Mutex;

    /// An in-memory store whose reads or writes can be made to fail.
    #[derive(Default)]
    struct Memory {
        saved: Mutex<CastCredentials>,
        fail_get: Mutex<Option<String>>,
        fail_set: Mutex<Option<String>>,
        /// Only this device's writes fail, when set.
        fail_set_only: Mutex<Option<String>>,
    }

    impl Store for Memory {
        fn get(&self, identifier: &str) -> Result<Option<DeviceCredentials>, String> {
            if let Some(e) = self.fail_get.lock().unwrap().clone() {
                return Err(e);
            }
            Ok(self.saved.lock().unwrap().get(identifier).cloned())
        }
        fn set(&self, identifier: &str, credentials: &DeviceCredentials) -> Result<(), String> {
            if let Some(e) = self.fail_set.lock().unwrap().clone() {
                return Err(e);
            }
            if self.fail_set_only.lock().unwrap().as_deref() == Some(identifier) {
                return Err("Credential Manager: write failed".into());
            }
            self.saved.lock().unwrap().insert(identifier.into(), credentials.clone());
            Ok(())
        }
    }

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
    fn saved_tv_a() -> Memory {
        let store = Memory::default();
        remember(&store, &mut CastCredentials::new(), "tv-a", creds(&[("AirPlay", "a1")])).unwrap();
        store
    }

    #[test]
    fn credentials_survive_a_restart_per_device() {
        let store = Memory::default();
        let mut session = CastCredentials::new();
        remember(&store, &mut session, "tv", creds(&[("AirPlay", "a1")])).unwrap();
        remember(&store, &mut session, "speaker", creds(&[("RAOP", "r1")])).unwrap();
        remember(&store, &mut session, "tv", creds(&[("RAOP", "r2")])).unwrap();
        remember(&store, &mut session, "tv", creds(&[("AirPlay", "a2")])).unwrap();
        // No session is what the app has after a restart.
        let fresh = CastCredentials::new();
        assert_eq!(credentials_for(&store, &fresh, "tv"), creds(&[("AirPlay", "a2"), ("RAOP", "r2")]));
        assert_eq!(credentials_for(&store, &fresh, "speaker"), creds(&[("RAOP", "r1")]));
        assert!(credentials_for(&store, &fresh, "never-paired").is_empty());
    }

    #[test]
    fn an_entry_that_cant_be_read_is_not_replaced_and_the_new_pairing_still_works() {
        let store = saved_tv_a();
        *store.fail_get.lock().unwrap() = Some("Credential Manager: access denied".into());
        let mut session = CastCredentials::new();
        let mut event = paired("tv-a", "a2");
        let log = on_sidecar_event(&store, &mut session, &mut event).unwrap();
        assert_eq!(event, json!({"id": 9, "event": "paired", "identifier": "tv-a", "saved": false}));
        assert!(log.contains("wasn't saved") && !log.contains("a2"), "{log}");
        assert_eq!(credentials_for(&store, &session, "tv-a")["AirPlay"], "a2", "usable until the app quits");
        *store.fail_get.lock().unwrap() = None;
        assert_eq!(store.get("tv-a").unwrap().unwrap()["AirPlay"], "a1", "the saved entry is left as it was");
    }

    #[test]
    fn a_failed_save_says_so_and_keeps_the_old_entries() {
        let store = saved_tv_a();
        *store.fail_set.lock().unwrap() = Some("Credential Manager: disk full".into());
        let mut session = CastCredentials::new();
        let mut event = paired("tv-b", "b1");
        let log = on_sidecar_event(&store, &mut session, &mut event).unwrap();
        assert_eq!(event["saved"], json!(false));
        assert!(log.contains("disk full") && !log.contains("b1"), "{log}");
        assert_eq!(credentials_for(&store, &CastCredentials::new(), "tv-a")["AirPlay"], "a1");
        assert_eq!(credentials_for(&store, &session, "tv-b")["AirPlay"], "b1");
    }

    #[test]
    fn a_saved_pairing_has_no_saved_flag_and_this_runs_pairings_win() {
        let store = saved_tv_a();
        let mut session = CastCredentials::new();
        let mut event = paired("tv-a", "a2");
        assert_eq!(on_sidecar_event(&store, &mut session, &mut event).as_deref(), Some("paired with tv-a"));
        assert!(event.get("saved").is_none());
        session.insert("tv-a".into(), creds(&[("AirPlay", "a3")])); // newer than the store, this run only
        assert_eq!(credentials_for(&store, &session, "tv-a")["AirPlay"], "a3");
        assert_eq!(on_sidecar_event(&store, &mut session, &mut json!({"event": "playing"})), None);
    }

    #[test]
    fn the_old_file_moves_into_the_store_once_and_is_deleted() {
        let path = temp_file("migrate");
        std::fs::write(&path, r#"{"tv": {"AirPlay": "a1", "RAOP": "r1"}, "speaker": {"RAOP": "s1"}}"#).unwrap();
        let store = Memory::default();
        store.set("tv", &creds(&[("AirPlay", "newer")])).unwrap(); // paired again since: the store wins
        let log = migrate(&path, &store).unwrap();
        assert!(log.contains("moved 2 pairing(s)"), "{log}");
        assert!(!path.exists(), "the plaintext file is gone");
        assert_eq!(store.get("tv").unwrap().unwrap(), creds(&[("AirPlay", "newer"), ("RAOP", "r1")]));
        assert_eq!(store.get("speaker").unwrap().unwrap(), creds(&[("RAOP", "s1")]));
        assert_eq!(migrate(&path, &store), None, "nothing left to move");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Like keyring's mock backend (what it picks off Windows): every write succeeds, nothing is kept.
    struct Forgetful;

    impl Store for Forgetful {
        fn get(&self, _: &str) -> Result<Option<DeviceCredentials>, String> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &DeviceCredentials) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_store_that_doesnt_keep_what_it_was_given_never_gets_the_file_deleted() {
        let path = temp_file("forgetful");
        let text = r#"{"tv": {"AirPlay": "a1"}}"#;
        std::fs::write(&path, text).unwrap();
        let log = migrate(&path, &Forgetful).unwrap();
        assert!(log.contains("kept") && log.contains("didn't keep") && !log.contains("a1"), "{log}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "the only copy of the pairings stays");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Off Windows the pairings stay in the file, with its guarantees: per device, written aside and
    /// moved into place, and a file that can't be parsed is never overwritten.
    #[test]
    fn the_file_store_keeps_each_devices_pairing_and_never_overwrites_a_bad_file() {
        let path = temp_file("file-store");
        let store = FileStore::new(path.clone());
        assert_eq!(store.get("tv").unwrap(), None);
        store.set("tv", &creds(&[("AirPlay", "a1")])).unwrap();
        store.set("speaker", &creds(&[("RAOP", "r1")])).unwrap();
        assert_eq!(store.get("tv").unwrap(), Some(creds(&[("AirPlay", "a1")])));
        assert_eq!(load_file(&path).unwrap().len(), 2);
        assert!(!path.with_extension("json.tmp").exists());

        std::fs::write(&path, "{not json").unwrap();
        assert!(store.get("tv").is_err());
        let mut event = paired("tv-b", "b1");
        on_sidecar_event(&store, &mut CastCredentials::new(), &mut event);
        assert_eq!(event["saved"], json!(false));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json", "the file is left as it was");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// TV A moves, TV B's write fails: the file stays, and TV B's pairing in it still works this run
    /// (the store's and this run's pairings win over the file's).
    #[test]
    fn pairings_left_in_the_file_by_an_unfinished_move_still_work() {
        let path = temp_file("fallback");
        std::fs::write(&path, r#"{"tv-a": {"AirPlay": "a1"}, "tv-b": {"AirPlay": "b1", "RAOP": "r1"}}"#).unwrap();
        let store = Memory::default();
        *store.fail_set_only.lock().unwrap() = Some("tv-b".into());
        assert!(migrate(&path, &store).unwrap().contains("kept"));
        assert!(path.exists());

        let with_file = LegacyFallback::new(&store, path.clone());
        let mut session = CastCredentials::new();
        assert_eq!(credentials_for(&with_file, &session, "tv-b"), creds(&[("AirPlay", "b1"), ("RAOP", "r1")]));
        assert_eq!(credentials_for(&with_file, &session, "tv-a")["AirPlay"], "a1");
        session.insert("tv-b".into(), creds(&[("AirPlay", "b2")]));
        assert_eq!(credentials_for(&with_file, &session, "tv-b"), creds(&[("AirPlay", "b2"), ("RAOP", "r1")]));

        // Once the move completes, the file is gone and the store answers alone.
        *store.fail_set_only.lock().unwrap() = None;
        migrate(&path, &store).unwrap();
        assert!(!path.exists());
        assert_eq!(credentials_for(&with_file, &CastCredentials::new(), "tv-b"), creds(&[("AirPlay", "b1"), ("RAOP", "r1")]));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_move_that_fails_keeps_the_file_for_next_time() {
        let path = temp_file("migrate-fails");
        let text = r#"{"tv": {"AirPlay": "a1"}}"#;
        std::fs::write(&path, text).unwrap();
        let store = Memory::default();
        *store.fail_set.lock().unwrap() = Some("Credential Manager: unavailable".into());
        let log = migrate(&path, &store).unwrap();
        assert!(log.contains("kept") && log.contains("unavailable") && !log.contains("a1"), "{log}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "the file is left as it was");

        *store.fail_set.lock().unwrap() = None;
        *store.fail_get.lock().unwrap() = Some("Credential Manager: access denied".into());
        assert!(migrate(&path, &store).unwrap().contains("kept"));
        assert!(path.exists(), "an unreadable store entry is never overwritten from the file");

        *store.fail_get.lock().unwrap() = None;
        migrate(&path, &store).unwrap();
        assert!(!path.exists());
        assert_eq!(store.get("tv").unwrap().unwrap()["AirPlay"], "a1");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_corrupt_file_is_kept_and_never_put_in_the_log() {
        const SENTINEL: &str = "SENTINEL-credential-7f3a91";
        let path = temp_file("sentinel");
        // Wrong shapes whose parse errors would quote the value, and broken JSON around it.
        for junk in [
            format!(r#"{{"tv-a": "{SENTINEL}"}}"#),
            format!(r#"{{"tv-a": {{"AirPlay": ["{SENTINEL}"]}}}}"#),
            format!(r#"{{"tv-a": {{"AirPlay": "{SENTINEL}" "#),
            format!(r#"["{SENTINEL}"]"#),
            String::new(),
        ] {
            std::fs::write(&path, &junk).unwrap();
            let log = migrate(&path, &Memory::default()).unwrap();
            assert!(!log.contains(SENTINEL), "log line quotes the file: {log}");
            assert!(log.contains("kept") && log.contains("line 1"), "says what and where: {log}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), junk, "the file is left as it was");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The file is there but can't be read (another program holds it open without sharing reads):
    /// nothing is moved and the file stays.
    #[cfg(windows)]
    #[test]
    fn a_file_that_cant_be_read_is_kept() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_DELETE: u32 = 4;
        let path = temp_file("read-denied");
        std::fs::write(&path, r#"{"tv-a": {"AirPlay": "a1"}}"#).unwrap();
        let holder = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_DELETE).open(&path).unwrap();
        let store = Memory::default();
        assert!(migrate(&path, &store).unwrap().contains("couldn't read"));
        drop(holder);
        assert!(path.exists());
        assert!(store.saved.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The real Credential Manager, under a namespace of this test's own: an entry round-trips, a
    /// device never paired reads as None, and a malformed entry is an error that doesn't quote it.
    #[cfg(windows)]
    #[test]
    fn credential_manager_keeps_each_devices_pairing() {
        const SENTINEL: &str = "SENTINEL-credential-7f3a91";
        let store = Keyring::new(&format!("open-airplay-cast-test-{}", std::process::id()));
        let tv = creds(&[("AirPlay", "a1"), ("RAOP", "r1")]);
        store.set("tv", &tv).unwrap();
        assert_eq!(store.get("tv").unwrap(), Some(tv));
        assert_eq!(store.get("speaker").unwrap(), None);
        store.entry("bad").unwrap().set_password(&format!(r#"{{"AirPlay": ["{SENTINEL}"]}}"#)).unwrap();
        let err = store.get("bad").unwrap_err();
        assert!(!err.contains(SENTINEL) && err.contains("isn't readable"), "{err}");
        for id in ["tv", "bad"] {
            store.delete(id).unwrap();
        }
        assert_eq!(store.get("tv").unwrap(), None);
    }

    #[test]
    fn casts_carry_only_their_own_devices_credentials() {
        let store = Memory::default();
        store.set("tv", &creds(&[("AirPlay", "a1")])).unwrap();
        let none = CastCredentials::new();
        let lookup = |id: &str| credentials_for(&store, &none, id);
        let mut cast = json!({"id": 1, "cmd": "play_url", "identifier": "tv", "url": "http://x"});
        attach(&mut cast, lookup);
        assert_eq!(cast["credentials"], json!({"AirPlay": "a1"}));

        let mut other = json!({"id": 2, "cmd": "stream_file", "identifier": "speaker", "path": "C:/a.mp3", "credentials": {"RAOP": "from the panel"}});
        attach(&mut other, lookup);
        assert!(other.get("credentials").is_none());

        let mut scan = json!({"id": 3, "cmd": "scan", "identifier": "tv"});
        attach(&mut scan, lookup);
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
