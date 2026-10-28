# Backups

## Making a backup

```
kkt backup ~/kkt-backups
```

This writes a new folder inside `~/kkt-backups` (created if missing), named
for the time and the kkt version that wrote it, such as
`kkt-20261028T131500.123Z-v0.4.0`. Names sort by time. The folder holds:

- `kashkeshet.db`: a copy of the database
- `config.toml`: a copy of the config file, if there is one
- `access-key`: a copy of the access key, if the server uses one
- `about.txt`: when the backup was made, by which version, and where each
  file came from

It is safe while the server runs. The database is copied with SQLite's
online backup, which gives the database as it was at one moment even while
agents write, and the copy is checked before the command finishes. Never back
up the database by copying its file: a copy made during a write can be
broken.

The folder and the key in it are readable only by you. Keep backups as
private as the key itself.

After `kkt setup`, `kkt backup` follows the saved mode: it backs up the
service's database or asks the managed Docker server to back up and copies
the result to the host folder you named. With Docker, `kkt backup` without
a folder keeps the result in the managed `docker/backups` folder only;
with a service or no setup record, the destination folder is required. Client-only machines refer you to
the server owner.

Without a setup record, `kkt backup` uses the database, config file and key
that `kkt serve` would use. Give `--db` or `--config` if the server uses
others.

## Before an upgrade

When a new version of kkt starts and finds a database made by an older one,
it upgrades the database's tables. First it backs up the database (only the
database, since nothing else changes) into `upgrade-backups` beside the
database file. It
keeps the newest 5 of these and removes older ones; set
`upgrade_backups_kept` (or `--upgrade-backups-kept`) to keep a different
number. Backups you make with `kkt backup` are never removed.

Such a backup is named for the new version, which made it, but holds the
database as the older version left it.

If that backup fails, for example because the disk is full, the server does
not start, says why, and leaves the database unchanged.

## Restoring

1. Stop the server.
2. Delete `kashkeshet.db-wal` and `kashkeshet.db-shm` beside the database
   file, if they exist. They belong to the database being replaced.
3. Copy the backup's `kashkeshet.db` over the database file.
4. Start the server.

To go back to an older version of kkt, restore a backup made by that version
or before, then start that version. A backup made before an upgrade has the
database as the older version left it. Do not run an older version on a
database a newer one has upgraded: it does not know the newer tables.

The backup's `config.toml` and `access-key` are there in case those files
are lost too; copy them back to the paths `about.txt` gives. A restored key
keeps clients that already use it connected.

For Docker, see [Docker](docker.md#backups).
