use super::*;
use chrono::TimeZone;

fn at(seconds: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 28, 13, 15, seconds)
        .single()
        .expect("time")
}

/// A fresh, empty directory for one test's files.
fn scratch_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-backup-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A database "copy" that writes the source's text, as the store's would
/// write its pages.
fn copy_text(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn names_say_the_version_and_sort_by_time() {
    assert_eq!(name(at(0), "0.3.0"), "kkt-20261028T131500.000Z-v0.3.0");
    let later = at(0) + chrono::Duration::milliseconds(1);
    assert!(name(at(0), "0.9.0") < name(later, "0.3.0"));
    assert!(is_backup(&name(at(0), "0.3.0")));
    for other in [
        "kkt-20261028T131500.000Z-v0.3.0.partial",
        "kkt-yesterday-v0.3.0",
        "kkt-20261028T131500.000Z",
        "notes.txt",
    ] {
        assert!(!is_backup(other), "{other}");
    }
}

#[test]
fn a_backup_holds_each_file_and_keeps_the_key_private() {
    let dir = scratch_dir("write");
    let (database, config, key) = (dir.join("live.db"), dir.join("c.toml"), dir.join("key"));
    std::fs::write(&database, "pages").expect("db");
    std::fs::write(&config, "bind = \"127.0.0.1:7420\"").expect("config");
    std::fs::write(&key, "secret").expect("key");
    let contents = Contents {
        database: &database,
        config: Some(&config),
        access_key: Some(&key),
        web_key: Some(&key),
    };

    let folder = dir.join("backups");
    let written = write(&folder, at(0), "0.3.0", &contents, copy_text).expect("backup");

    assert_eq!(written, folder.join("kkt-20261028T131500.000Z-v0.3.0"));
    let read = |file: &str| std::fs::read_to_string(written.join(file)).expect(file);
    assert_eq!(read(DATABASE_FILE), "pages");
    assert_eq!(read(CONFIG_FILE), "bind = \"127.0.0.1:7420\"");
    assert_eq!(read(ACCESS_KEY_FILE), "secret");
    assert_eq!(read(WEB_KEY_FILE), "secret");
    assert_eq!(
        read(ABOUT_FILE),
        format!(
            "kkt 0.3.0 backup made 2026-10-28T13:15:00Z\n\
             kashkeshet.db: copy of {}\n\
             config.toml: copy of {}\n\
             access-key: copy of {}\n\
             web-key: copy of {}\n",
            database.display(),
            config.display(),
            key.display(),
            key.display()
        )
    );
    #[cfg(unix)]
    {
        assert_eq!(mode(&written), 0o700);
        assert_eq!(mode(&written.join(ACCESS_KEY_FILE)), 0o600);
        assert_eq!(mode(&written.join(WEB_KEY_FILE)), 0o600);
    }
    // The same moment again would overwrite it, so it is refused.
    assert!(write(&folder, at(0), "0.3.0", &contents, copy_text).is_err());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn without_config_or_key_only_the_database_is_backed_up() {
    let dir = scratch_dir("db-only");
    let database = dir.join("live.db");
    std::fs::write(&database, "pages").expect("db");
    let contents = Contents {
        database: &database,
        config: None,
        access_key: None,
        web_key: None,
    };
    let written = write(&dir.join("b"), at(0), "0.3.0", &contents, copy_text).expect("backup");
    let mut files: Vec<_> = std::fs::read_dir(&written)
        .expect("list")
        .map(|e| e.expect("entry").file_name().into_string().expect("name"))
        .collect();
    files.sort();
    assert_eq!(files, [ABOUT_FILE, DATABASE_FILE]);
    let about = std::fs::read_to_string(written.join(ABOUT_FILE)).expect("about");
    let lines: Vec<&str> = about.lines().skip(1).collect();
    assert_eq!(
        lines,
        [
            format!("kashkeshet.db: copy of {}", database.display()).as_str(),
            "config.toml: none",
            "access-key: none",
            "web-key: none",
        ]
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_failed_backup_leaves_nothing_behind() {
    let dir = scratch_dir("fail");
    let database = dir.join("live.db");
    std::fs::write(&database, "pages").expect("db");
    let contents = Contents {
        database: &database,
        config: None,
        // Missing, so copying it fails after the database is copied.
        access_key: Some(&dir.join("no-such-key")),
        web_key: None,
    };
    let folder = dir.join("backups");
    let error = write(&folder, at(0), "0.3.0", &contents, copy_text).expect_err("fails");
    assert!(error.contains("no-such-key"), "{error}");
    assert_eq!(std::fs::read_dir(&folder).expect("list").count(), 0);

    let failing = |_: &Path, _: &Path| Err("disk full".to_string());
    let error = write(&folder, at(1), "0.3.0", &contents, failing).expect_err("fails");
    assert!(error.contains("disk full"), "{error}");
    assert_eq!(std::fs::read_dir(&folder).expect("list").count(), 0);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn pruning_keeps_the_newest_backups_and_nothing_else_is_touched() {
    let dir = scratch_dir("prune");
    let backups: Vec<String> = (0..5).map(|s| name(at(s), "0.3.0")).collect();
    for backup in &backups {
        std::fs::create_dir(dir.join(backup)).expect("backup");
    }
    std::fs::write(dir.join("notes.txt"), "mine").expect("other file");
    let partial = format!("{}{PARTIAL}", name(at(9), "0.3.0"));
    std::fs::create_dir(dir.join(&partial)).expect("partial");

    let removed = prune(&dir, 2).expect("prune");

    assert_eq!(
        removed,
        [
            dir.join(&backups[0]),
            dir.join(&backups[1]),
            dir.join(&backups[2])
        ]
    );
    let mut left: Vec<_> = std::fs::read_dir(&dir)
        .expect("list")
        .map(|e| e.expect("entry").file_name().into_string().expect("name"))
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            backups[3].clone(),
            backups[4].clone(),
            partial,
            "notes.txt".into()
        ]
    );
    assert!(
        prune(&dir.join("missing"), 2)
            .expect("missing folder")
            .is_empty()
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
