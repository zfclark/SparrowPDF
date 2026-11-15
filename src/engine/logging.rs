//! Minimal file logging, so "nothing was written" reports can be diagnosed.
//!
//! Lines are appended to `%LOCALAPPDATA%\sparrowpdf\sparrowpdf.log` (falling
//! back to the system temp dir). Logging never panics and never fails the
//! caller: a logging problem must never turn into a second error on top of
//! the one being reported.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Serialises appends inside this process.
///
/// Append mode gives each `File` its own handle, so two threads writing one
/// record each can still interleave on Windows. The lock covers opening and
/// writing the whole line; a poisoned lock is recovered instead of dropping the
/// record, because logging must never fail the caller.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Append one line to the app log. Best-effort by design.
pub fn log(kind: &str, message: &str) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    append_line(&path, &format!("{} [{}] {}\n", utc_now(), kind, message));
}

/// Append one already-formatted record to `path`.
///
/// One `write_all` per record: formatting straight into the file issues several
/// writes, which a concurrent logger (the worker pool logs per folder) can split
/// in two. The mutex keeps whole records from interleaving across threads.
fn append_line(path: &Path, line: &str) {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
}

/// Where the log lives (also the path to give users for support requests).
///
/// `SPARROW_LOG_DIR` overrides the location (used by tests and useful for
/// portable setups); the default is the per-user AppData directory.
pub fn log_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("SPARROW_LOG_DIR") {
        return PathBuf::from(dir).join("sparrowpdf.log");
    }
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(dir).join("sparrowpdf").join("sparrowpdf.log");
    }
    std::env::temp_dir().join("sparrowpdf.log")
}

/// UTC timestamp without pulling in a time dependency.
fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days`: epoch days -> (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn scratch(tag: &str) -> PathBuf {
        let base = match option_env!("CARGO_TARGET_TMPDIR") {
            Some(base) => PathBuf::from(base),
            None => {
                let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("target")
                    .join("tmp");
                let _ = std::fs::create_dir_all(&base);
                base
            }
        };
        let dir = base.join(format!("sparrowpdf-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Concurrent appends must not split or interleave a record.
    #[test]
    fn concurrent_appends_keep_records_whole() {
        let dir = scratch("parallel");
        let path = Arc::new(dir.join("app.log"));
        let threads: Vec<_> = (0..8)
            .map(|thread| {
                let path = Arc::clone(&path);
                std::thread::spawn(move || {
                    for index in 0..50 {
                        append_line(&path, &format!("T{thread} #{index:02}\n"));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("writer thread");
        }

        let content = std::fs::read_to_string(path.as_path()).expect("read log");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 400, "every record is on its own line");
        for line in lines {
            let (thread, index) = line
                .split_once(" #")
                .unwrap_or_else(|| panic!("record was mangled: {line:?}"));
            assert!(thread.starts_with('T') && thread.len() == 2, "thread tag: {line:?}");
            assert!(index.len() == 2, "sequence number: {line:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
