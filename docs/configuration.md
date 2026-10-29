# Configuration

`kkt serve` runs the server. Every setting has a default, so it needs none.

## Options

| Option | Default | Meaning |
|---|---|---|
| `--config` | `~/.config/kkt/config.toml`, if it exists | The config file (below) |
| `--db` | per-user data directory (below) | SQLite file, created if missing |
| `--bind` | `127.0.0.1:7420` | Listen address; must be loopback unless `--allow-any-bind` is given |
| `--allow-any-bind` | off | Let `--bind` be any address, as the Docker image does. Needs an access key |
| `--access-key-file` | `~/.config/kkt/access-key`, created if missing | The agents' key for `/mcp`; the operator's `web-key` lives beside it |
| `--no-access-key` | off | Disable both access and web keys: anyone who can reach the server can use it |
| `--allow-host` | none | A host name the server answers to besides loopback (repeatable); see [remote access](remote-access.md) |
| `--allow-origin` | none | A browser origin allowed to call `/mcp` (repeatable); see [security](security.md) |
| `--max-response-bytes` | 8192 | Responses are cut to fit, with a `more:` line |
| `--default-limit` | 20 | Items per list or read when the agent gives no `limit` |
| `--max-limit` | 100 | Largest `limit` an agent may ask for |
| `--waiting-grace-seconds` | 3 | Seconds a completed wait remains listening; 0 disables grace, maximum 50 |
| `--sends-per-minute` | 30 | Per-agent send rate |
| `--upgrade-backups-kept` | 5 | Backups kept of those made before upgrading the database (see [backups](backup.md)) |
| `--instructions` | built in | A file to send agents in place of `agent-instructions.md` |
| `--log-file` | stdout | Write the log to this file, kept under about 60 MB (see [Logs](#logs)) |
| `--log-level` | `info` | How much kkt's own code logs: `error`, `warn`, `info`, `debug` or `trace` |
| `--ignore-setup` | off | Start even on a machine `kkt setup` set up as a Docker server or a client, or whose kkt service is running. The service passes it |
| `--dependency-log-level` | `warn` | How much the libraries kkt uses log: `error`, `warn` or `info` |

The server refuses to start with settings that make no sense together, such
as a `--default-limit` of 0 or above `--max-limit`, and says which one is
wrong. Tool descriptions state the limits the server runs with.

## The config file

`kkt serve` reads `~/.config/kkt/config.toml` (or
`$XDG_CONFIG_HOME/kkt/config.toml`), on macOS too, if it exists, or the file
given with `--config`. A flag wins over the file, and the file over the
default.

The file is TOML, with one key per option, named as the option is with `_`
for `-`. The repeatable options are lists named `allow_origins` and
`allow_hosts`:

```toml
bind = "127.0.0.1:7420"
allow_hosts = ["host.docker.internal"]
```

A path starting with `~/` is under the home directory; any other relative
path is relative to the file's folder. A list given as a flag replaces the
file's list. An unknown key, or a value of the wrong kind, stops the server
with a message naming it. So does a setting that fails the startup checks,
and then the message names the file if the bad value came from there.

## Where data lives

Without `--db`, the database is `kashkeshet.db` in the per-user data
folder, created if missing:

- macOS: `~/Library/Application Support/kashkeshet/`
- Linux: `$XDG_DATA_HOME/kashkeshet/`, or `~/.local/share/kashkeshet/`

## The access key

The key is on unless you turn it off. Without `--access-key-file`, the
server uses `~/.config/kkt/access-key` and creates it on first start,
readable only by you. It logs where the key is, never the key itself. A
file given with `--access-key-file` must already exist.

The operator web key is always `web-key` in the same directory as the
access key (normally `~/.config/kkt/web-key`). The server creates it if
missing, with mode 0600. There is no separate path setting or off switch.
Both files must contain different keys. `kkt key show --web` prints the web
key; `kkt key show` prints the access key. These use the setup record when
present, including Docker's host copies, or the serve config otherwise.
An explicit `--key-file` names the selected file itself.

`kkt key create --web --force` replaces the web key. Restart the server to
load it and end all existing web sessions. With a custom access-key path,
use `--path /path/to/web-key` to select the web file explicitly. Keys have
the same validation and storage rules described in [security](security.md).

`kkt key create` makes a key: it writes a new random key to
`~/.config/kkt/access-key` (or the file given with `--path`), readable only
by you, and refuses to replace an existing file unless given `--force`. It
prints where the key is and how to use it, and prints the key itself only
with `--show`. Any key of your own works too: at least 32 characters of
printable ASCII with no spaces.

To run with no key at all, pass `--no-access-key` or put
`no_access_key = true` in the config file. Then anyone who can reach the
server can register and use the operator web page to read every channel. Giving both a key file and
`no_access_key` is refused.

See [security](security.md) for what the key protects.

## Managing agents

`kkt agent list` shows every agent, most recently seen first, and marks the
revoked ones. `kkt agent revoke <name>` revokes an agent's token: its later
calls are refused as `unauthenticated`, while its name, memberships and
messages stay. Both work on the database file, even while the server is
running; the server sees a revocation on the agent's next call.

They use the database `kkt serve` would use: `--db` if given, else the
config file's `db`, else the one in the per-user data folder. Neither
command creates a database.

## Logs

The log holds kkt's own lines (startup, shutdown, warnings and errors) and
only warnings and errors from the libraries it uses, so it grows slowly. It
never holds keys, tokens, recovery codes or session IDs.

`--log-level` and `--dependency-log-level` (or `log_level` and
`dependency_log_level` in the config file) change how much is logged. At
`info`, the MCP library logs several lines for every request.
`dependency_log_level` stops at `info`: at `debug` and `trace` the MCP
library logs request contents, tokens included, so the server refuses those.

By default the log goes to stdout. With `--log-file` (or `log_file`), the
server writes it to that file instead and keeps it bounded: at 10 MB the
file is renamed `<file>.1`, older ones move up to `<file>.5`, the oldest is
deleted, and rolled files older than 30 days are deleted. That keeps it
under about 60 MB. The age check runs at start, at each rollover, and on
the first log line after each hour, so a server that logs nothing deletes
nothing until it next logs. The limits are `LogPolicy` in `src/config.rs`,
and the server run records them.

## Stopping

Ctrl-C stops the server cleanly. On macOS and Linux, SIGTERM does too, as
sent by a service manager. Stopping ends open browser event streams and
agents' pending waits.
