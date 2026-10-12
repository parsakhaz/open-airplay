//! The engine's log file (`engine.log`): capped in size, with a few older files kept beside it
//! as `engine.1.log` (newest) to `engine.<keep>.log` (oldest).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Size cap for `engine.log`, and how many older files to keep beside it.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const KEEP: usize = 3;

/// The app's engine log in `dir`, shared by every receiver run. The first call opens it,
/// which moves the previous app run's log aside.
pub fn shared(dir: &Path) -> Option<Arc<RotatingLog>> {
    static LOG: OnceLock<Option<Arc<RotatingLog>>> = OnceLock::new();
    LOG.get_or_init(|| {
        fs::create_dir_all(dir).ok()?;
        RotatingLog::open(&dir.join("engine.log"), MAX_BYTES, KEEP).ok().map(Arc::new)
    })
    .clone()
}

pub struct RotatingLog {
    inner: Mutex<Inner>,
}

struct Inner {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    /// None only if reopening after a rotation failed; lines are then dropped.
    file: Option<File>,
    len: u64,
}

impl RotatingLog {
    /// Opens `path` for appending, first moving any earlier run's log aside.
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        if fs::metadata(path).is_ok_and(|m| m.len() > 0) {
            shift(path, keep);
        }
        let file = append(path)?;
        let len = file.metadata()?.len();
        Ok(RotatingLog { inner: Mutex::new(Inner { path: path.to_path_buf(), max_bytes, keep, file: Some(file), len }) })
    }

    /// Appends one line. Logging is best effort: write errors are ignored.
    pub fn write_line(&self, line: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let bytes = line.len() as u64 + 1;
        if inner.len > 0 && inner.len + bytes > inner.max_bytes {
            inner.rotate();
        }
        if let Some(file) = inner.file.as_mut() {
            if writeln!(file, "{line}").is_ok() {
                inner.len += bytes;
            }
        }
    }
}

impl Inner {
    fn rotate(&mut self) {
        self.file = None; // Windows can't rename a file that is still open.
        shift(&self.path, self.keep);
        self.file = append(&self.path).ok();
        self.len = 0;
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// `<stem>.<n>.<ext>` beside `path`, e.g. `engine.2.log`.
fn numbered(path: &Path, n: usize) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    match path.extension() {
        Some(ext) => path.with_file_name(format!("{stem}.{n}.{}", ext.to_string_lossy())),
        None => path.with_file_name(format!("{stem}.{n}")),
    }
}

/// Moves `path` to `<stem>.1`, each older file up one number, and drops the one past `keep`.
fn shift(path: &Path, keep: usize) {
    if keep == 0 {
        let _ = fs::remove_file(path);
        return;
    }
    let _ = fs::remove_file(numbered(path, keep));
    for n in (1..keep).rev() {
        let _ = fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    let _ = fs::rename(path, numbered(path, 1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oap-engine-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        fs::read_to_string(dir.join(name)).ok()
    }

    #[test]
    fn startup_moves_the_previous_runs_log_aside() {
        let dir = dir("startup");
        fs::write(dir.join("engine.log"), "previous run\n").unwrap();
        let log = RotatingLog::open(&dir.join("engine.log"), 1024, 3).unwrap();
        log.write_line("this run");
        assert_eq!(read(&dir, "engine.log").as_deref(), Some("this run\n"));
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("previous run\n"));
    }

    #[test]
    fn a_full_log_rotates_before_the_line_that_would_overflow_it() {
        let dir = dir("size");
        let log = RotatingLog::open(&dir.join("engine.log"), 20, 3).unwrap();
        log.write_line("first 123"); // 10 bytes
        log.write_line("second 12"); // 20 bytes: exactly at the cap, still one file
        log.write_line("third 123"); // would make 30
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("first 123
second 12
"));
        assert_eq!(read(&dir, "engine.log").as_deref(), Some("third 123
"));
    }

    #[test]
    fn keeps_only_the_newest_old_files() {
        let dir = dir("keep");
        let log = RotatingLog::open(&dir.join("engine.log"), 5, 2).unwrap();
        for line in ["log1", "log2", "log3", "log4"] {
            log.write_line(line); // 5 bytes each: every line after the first rotates
        }
        assert_eq!(read(&dir, "engine.log").as_deref(), Some("log4
"));
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("log3
"));
        assert_eq!(read(&dir, "engine.2.log").as_deref(), Some("log2
"));
        assert_eq!(read(&dir, "engine.3.log"), None);
    }
}
