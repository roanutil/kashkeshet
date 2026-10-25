# Running in the background

`kkt service install` runs `kkt serve` as a service of your login: a
launchd user agent on macOS, a systemd user unit on Linux. It starts the
server now and at each login, and starts it again only if it fails. A clean
stop stays stopped.

```
kkt service install            # write the service file and start it
kkt service install --dry-run  # show what would be written and run
kkt service status
kkt service logs --follow
kkt service stop               # until `kkt service start` or the next login
kkt service uninstall          # the database, config and logs stay
```

## What the service runs

Services start in a folder of the manager's choosing, so the service runs
this program with absolute paths: the config file if there is one, and the
database and access key file unless the config file names them. With no key
set anywhere, `install` uses `~/.config/kkt/access-key`, and makes it first
if it is missing. `install` also takes `--config`, `--db`,
`--access-key-file` and `--no-access-key`, as `serve` does.

Everything else comes from the config file each time the service starts.
After editing it, run `kkt service stop` and then `kkt service start`.
Install again after moving the program or changing `--db`.

`install` checks the settings first, so a bad one fails there rather than in
a loop of restarts.

## Where things go

- **macOS:** `~/Library/LaunchAgents/com.sam-mfb.kashkeshet.plist`, with the
  log in `~/Library/Logs/kashkeshet/kashkeshet.log`. The log is not rotated.
- **Linux:** `~/.config/systemd/user/kashkeshet.service` (or
  `$XDG_CONFIG_HOME/systemd/user/`), with the log in the systemd journal. A
  user unit stops when you log out unless you run `loginctl enable-linger`.
