//! A log file the server writes itself, kept within a [`LogPolicy`]: rolled
//! over by size, with a bounded number of rolled files, none kept longer
//! than the policy's age. Used where nothing else bounds the log, such as a
//! launchd service on macOS.

use crate::clock::Clock;
use crate::config::{LOG_PRUNE_INTERVAL, LogPolicy};
use chrono::{DateTime, Utc};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The log file at `path`, appended to, and rolled over by its policy.
pub struct RotatingFile {
    path: PathBuf,
    policy: LogPolicy,
    clock: Arc<dyn Clock>,
    file: File,
    size: u64,
    /// When rolled files were last checked for age.
    pruned_at: DateTime<Utc>,
}

impl RotatingFile {
    /// Opens `path` for appending, creating it and its folder if needed, and
    /// deletes rolled files that are past the policy's age.
    pub fn open(path: &Path, policy: LogPolicy, clock: Arc<dyn Clock>) -> io::Result<RotatingFile> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file = append(path)?;
        let size = file.metadata()?.len();
        let pruned_at = clock.now();
        let mut log = RotatingFile {
            path: path.to_path_buf(),
            policy,
            clock,
            file,
            size,
            pruned_at,
        };
        log.prune()?;
        Ok(log)
    }

    /// `<file>.<n>`, the `n`th most recently rolled file.
    fn rolled(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// Renames the file to `<file>.1`, moving older ones up and dropping the
    /// oldest, then starts a new, empty file.
    fn roll(&mut self) -> io::Result<()> {
        self.file.flush()?;
        remove_if_present(&self.rolled(self.policy.rotated_files))?;
        for n in (1..self.policy.rotated_files).rev() {
            rename_if_present(&self.rolled(n), &self.rolled(n + 1))?;
        }
        std::fs::rename(&self.path, self.rolled(1))?;
        self.file = append(&self.path)?;
        self.size = 0;
        self.prune()
    }

    /// Deletes rolled files last written more than the policy's age ago.
    fn prune(&mut self) -> io::Result<()> {
        let max_age = chrono::Duration::from_std(self.policy.max_age)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let now = self.clock.now();
        self.pruned_at = now;
        let cutoff = now - max_age;
        for n in 1..=self.policy.rotated_files {
            let rolled = self.rolled(n);
            let written = match std::fs::metadata(&rolled) {
                Ok(meta) => DateTime::<Utc>::from(meta.modified()?),
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if written < cutoff {
                remove_if_present(&rolled)?;
            }
        }
        Ok(())
    }
}

impl Write for RotatingFile {
    /// Writes `buf` to the file, rolling it over first if `buf` would take
    /// it past the policy's size. A file is never rolled while empty, so one
    /// write larger than the limit still lands whole. Otherwise, at most
    /// once per [`LOG_PRUNE_INTERVAL`], deletes rolled files past their age.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.policy.max_bytes {
            self.roll()?;
        } else if self.clock.now() - self.pruned_at
            >= chrono::Duration::from_std(LOG_PRUNE_INTERVAL)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
        {
            self.prune()?;
        }
        let written = self.file.write(buf)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn rename_if_present(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::testing::FixedClock;
    use std::time::Duration;

    const POLICY: LogPolicy = LogPolicy {
        max_bytes: 100,
        rotated_files: 3,
        max_age: Duration::from_secs(24 * 60 * 60),
    };

    /// A fresh folder, a clock, and the log path in it. The clock is set in
    /// 2000, so files written now count as new; a test makes a file old by
    /// setting its modification time before the clock (see `make_old`).
    fn setup(name: &str) -> (PathBuf, Arc<FixedClock>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("kashkeshet-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let start = chrono::TimeZone::with_ymd_and_hms(&Utc, 2000, 1, 1, 0, 0, 0).unwrap();
        let clock = Arc::new(FixedClock::new(start));
        let path = dir.join("logs/kkt.log");
        (dir, clock, path)
    }

    /// Sets `path` as last written `hours` before the clock's time.
    fn make_old(path: &Path, clock: &FixedClock, hours: i64) {
        let when = clock.now() - chrono::Duration::hours(hours);
        let file = OpenOptions::new().write(true).open(path).expect("open");
        file.set_modified(when.into()).expect("set modified");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    fn line(n: usize) -> Vec<u8> {
        // 40 bytes, so two fit in 100 and a third rolls the file.
        format!("{n:038}\n\n").into_bytes()[..40].to_vec()
    }

    #[test]
    fn the_file_rolls_over_at_the_size_limit() {
        let (dir, clock, path) = setup("size");
        let mut log = RotatingFile::open(&path, POLICY, clock).expect("open");
        for n in 0..3 {
            log.write_all(&line(n)).expect("write");
        }
        let rolled = log.rolled(1);
        assert_eq!(std::fs::metadata(&rolled).expect("rolled").len(), 80);
        assert_eq!(read(&path).len(), 40);
        assert!(read(&path).starts_with(&format!("{:038}", 2)));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn only_so_many_rolled_files_are_kept() {
        let (dir, clock, path) = setup("count");
        let mut log = RotatingFile::open(&path, POLICY, clock).expect("open");
        // Each pair of lines fills a file; twelve lines make five rollovers.
        for n in 0..12 {
            log.write_all(&line(n)).expect("write");
        }
        for n in 1..=POLICY.rotated_files {
            assert!(log.rolled(n).exists(), "{n}");
        }
        assert!(!log.rolled(POLICY.rotated_files + 1).exists());
        // The newest rolled file holds the lines just before the current ones.
        assert!(read(&log.rolled(1)).starts_with(&format!("{:038}", 8)));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn rolled_files_past_the_age_limit_are_deleted() {
        let (dir, clock, path) = setup("age");
        let mut log = RotatingFile::open(&path, POLICY, clock.clone()).expect("open");
        for n in 0..3 {
            log.write_all(&line(n)).expect("write");
        }
        // The rolled file was last written a day and an hour ago. At the
        // next rollover it moves to .2 and, being too old, is deleted; the
        // file just rolled stays.
        make_old(&log.rolled(1), &clock, 25);
        for n in 3..5 {
            log.write_all(&line(n)).expect("write");
        }
        assert!(!log.rolled(2).exists());
        assert!(log.rolled(1).exists());
        // A rolled file just under the age stays.
        make_old(&log.rolled(1), &clock, 23);
        drop(log);
        let log = RotatingFile::open(&path, POLICY, clock.clone()).expect("reopen");
        assert!(log.rolled(1).exists());
        // Opening deletes one past the age.
        make_old(&log.rolled(1), &clock, 25);
        drop(log);
        let log = RotatingFile::open(&path, POLICY, clock).expect("reopen");
        assert!(!log.rolled(1).exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_running_log_deletes_old_files_without_rolling_over() {
        let (dir, clock, path) = setup("running");
        let mut log = RotatingFile::open(&path, POLICY, clock.clone()).expect("open");
        for n in 0..3 {
            log.write_all(&line(n)).expect("write");
        }
        let rolled = log.rolled(1);
        // The rolled file goes past its age while the server keeps running.
        // A small write within the hour leaves it; one after the hour, still
        // well under the size limit, deletes it.
        make_old(&rolled, &clock, 25);
        clock.advance(chrono::Duration::minutes(59));
        log.write_all(b"x\n").expect("write");
        assert!(rolled.exists());
        clock.advance(chrono::Duration::minutes(1));
        log.write_all(b"x\n").expect("write");
        assert!(!rolled.exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn reopening_appends_and_counts_what_is_there() {
        let (dir, clock, path) = setup("reopen");
        let mut log = RotatingFile::open(&path, POLICY, clock.clone()).expect("open");
        log.write_all(&line(0)).expect("write");
        log.write_all(&line(1)).expect("write");
        drop(log);
        let mut log = RotatingFile::open(&path, POLICY, clock).expect("reopen");
        log.write_all(&line(2)).expect("write");
        // The two earlier lines count toward the limit, so the third rolls.
        assert_eq!(std::fs::metadata(log.rolled(1)).expect("rolled").len(), 80);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_write_larger_than_the_limit_lands_whole() {
        let (dir, clock, path) = setup("large");
        let mut log = RotatingFile::open(&path, POLICY, clock).expect("open");
        let big = vec![b'x'; 150];
        log.write_all(&big).expect("write");
        assert_eq!(read(&path).len(), 150);
        assert!(!log.rolled(1).exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
