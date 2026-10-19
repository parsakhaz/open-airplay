//! The app's log files (`engine.log`, `sender.log`, `mirror.log`): each capped in size, with a few
//! older files kept beside it as `<name>.1.log` (newest) to `<name>.<keep>.log` (oldest).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

/// Size cap for each log, and how many older files to keep beside it.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const KEEP: usize = 3;

/// The log at `path`, shared by everything that writes to it during this app run. The first call
/// for a path opens it, which moves the previous app run's log aside.
pub fn shared(path: &Path) -> Option<Arc<RotatingLog>> {
    static LOGS: OnceLock<Mutex<HashMap<PathBuf, Arc<RotatingLog>>>> = OnceLock::new();
    let mut logs = LOGS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(log) = logs.get(path) {
        return Some(log.clone());
    }
    fs::create_dir_all(path.parent()?).ok()?;
    let log = Arc::new(RotatingLog::open(path, MAX_BYTES, KEEP).ok()?);
    logs.insert(path.to_path_buf(), log.clone());
    Some(log)
}

/// Copies a child process's output (its stderr) into `log` a line at a time, on its own thread,
/// until the output ends. The child isn't handed the file itself, so the log can still rotate.
pub fn copy_lines(output: impl Read + Send + 'static, log: Arc<RotatingLog>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => log.write_line(String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n'])),
            }
        }
    })
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
            let _ = trim_to_tail(&numbered(path, 1), max_bytes);
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

/// Cuts `path` down to its last whole lines within `max_bytes`, if it is larger.
fn trim_to_tail(path: &Path, max_bytes: u64) -> io::Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= max_bytes {
        return Ok(());
    }
    // One byte more than the tail, so a cut that lands on a line start keeps that line.
    file.seek(SeekFrom::Start(len - max_bytes - 1))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    drop(file);
    let start = tail.iter().position(|&b| b == b'\n').map_or(tail.len(), |i| i + 1);
    fs::write(path, &tail[start..])
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
    fn one_log_per_path_is_shared_and_each_rotates_on_its_own() {
        let dir = dir("shared");
        fs::write(dir.join("sender.log"), "previous run
").unwrap();
        let a = shared(&dir.join("sender.log")).unwrap();
        let b = shared(&dir.join("sender.log")).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "the same file is opened once per app run");
        let mirror = shared(&dir.join("mirror.log")).unwrap();
        a.write_line("cast");
        mirror.write_line("mirror");
        assert_eq!(read(&dir, "sender.log").as_deref(), Some("cast
"));
        assert_eq!(read(&dir, "sender.1.log").as_deref(), Some("previous run
"));
        assert_eq!(read(&dir, "mirror.log").as_deref(), Some("mirror
"));
    }

    #[test]
    fn a_child_processes_output_is_copied_line_by_line_and_rotates() {
        let dir = dir("copy");
        let log = Arc::new(RotatingLog::open(&dir.join("mirror.log"), 12, 3).unwrap());
        copy_lines(std::io::Cursor::new(b"line one
line two
no newline".to_vec()), log.clone()).join().unwrap();
        assert_eq!(read(&dir, "mirror.2.log").as_deref(), Some("line one
"));
        assert_eq!(read(&dir, "mirror.1.log").as_deref(), Some("line two
"));
        assert_eq!(read(&dir, "mirror.log").as_deref(), Some("no newline
"));
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
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("first 123\nsecond 12\n"));
        assert_eq!(read(&dir, "engine.log").as_deref(), Some("third 123\n"));
    }

    #[test]
    fn keeps_only_the_newest_old_files() {
        let dir = dir("keep");
        let log = RotatingLog::open(&dir.join("engine.log"), 5, 2).unwrap();
        for line in ["log1", "log2", "log3", "log4"] {
            log.write_line(line); // 5 bytes each: every line after the first rotates
        }
        assert_eq!(read(&dir, "engine.log").as_deref(), Some("log4\n"));
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("log3\n"));
        assert_eq!(read(&dir, "engine.2.log").as_deref(), Some("log2\n"));
        assert_eq!(read(&dir, "engine.3.log"), None);
    }

    /// A log left by a build without rotation can be any size; only its last lines are kept.
    #[test]
    fn startup_trims_an_oversized_previous_log_to_its_last_whole_lines() {
        let dir = dir("trim");
        fs::write(dir.join("engine.log"), "line1\nline2\nline3\n").unwrap();
        RotatingLog::open(&dir.join("engine.log"), 10, 3).unwrap();
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("line3\n"));

        // A cut that lands exactly on a line start keeps that line.
        let dir = self::dir("trim-aligned");
        fs::write(dir.join("engine.log"), "line1\nline2\nline3\n").unwrap();
        RotatingLog::open(&dir.join("engine.log"), 12, 3).unwrap();
        assert_eq!(read(&dir, "engine.1.log").as_deref(), Some("line2\nline3\n"));
    }
}
