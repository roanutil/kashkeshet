# Setup

After installing the `kkt` program, run:

```sh
kkt setup
```

Choose a server as a service, a server in Docker, or a client-only
machine. Setup offers the clients it detects, checks the server connection
with `kkt doctor`, then applies the chosen clients' settings through
`kkt connect`.
If a step fails, the report explains what to fix; setup may already have
started a server or changed a client configuration. Restart connected
clients so they load their settings.

## Server on this machine

**Service** uses launchd on macOS or a systemd user service on Linux,
with the usual config file, database and key paths. It starts now and at
login; see [service](service.md) for logs and start/stop commands. A Linux
user service normally stops at logout unless lingering is enabled.

**Docker** needs the Docker daemon and Compose. Setup keeps its Compose
files and settings in a managed `docker` folder under kkt's data directory:
`~/Library/Application Support/kashkeshet/docker` on macOS, or
`~/.local/share/kashkeshet/docker` on Linux (`$XDG_DATA_HOME` if set).
The files are embedded in the installed binary for its version. Setup
creates the backups folder, handles its permissions, starts the containers
and saves their key in `~/.config/kkt/docker-access-key`, readable only by
you. No Compose download, version entry or manual key copy is needed.
[Docker](docker.md) describes the separate manual setup.

Without Tailscale, the web page is at <http://127.0.0.1:7420/> and clients
use `http://127.0.0.1:7420/mcp`.

## Tailscale

For a Docker server, setup starts a Tailscale container and shows a login
link. At a terminal it says "Press Enter to open it in your browser (or open
it yourself)": Enter opens the link with `open` (macOS) or `xdg-open`
(Linux), and setup never opens a browser unless you ask. If opening fails,
setup says so and the link stays on the screen. With no terminal, as in a
script, it only prints the link. Sign in with the link; setup keeps watching
for the login while it waits for Enter, so signing in some other way works
too. Setup waits up to ten minutes; if interrupted or
timed out, run setup again to resume with the saved container state. It
reads the node's actual DNS name, including any suffix Tailscale adds when
a name is already taken, and configures kkt to accept it.

### When the machine needs to log in again

A Tailscale machine's key expires after 180 days unless expiry is turned off
for it in the Tailscale admin console (see [Docker](docker.md#with-tailscale)).
When the key expires, or the machine is logged out, it drops off the tailnet,
and its HTTPS address stops answering, while kkt keeps running on this
machine. `kkt status` then prints `Tailscale login: needed`, and `kkt doctor`
reports a problem on the line "this machine's Tailscale login". A Tailscale
container that does not answer at all is reported as unknown, not as logged
out.

To log in again, run `kkt setup` again with the same answers. Setup notices
the machine is logged out, restarts the Tailscale container to get a fresh
login link, shows it as above (Enter opens it), and waits for you to log in.
It is the same machine: the same name, the same address, and nothing is
lost. There is no separate command for this.

For a service, install Tailscale on the host and log in there first. Setup
uses that login and `tailscale serve`; it does not log you out or replace
the host's Tailscale settings.

HTTPS certificates must be enabled on the tailnet. If setup reports they
are off, open the Tailscale admin console's DNS page, enable HTTPS
certificates, and rerun setup. Other machines use the HTTPS MCP URL setup
prints and a copy of the server's key. Use Tailscale's access controls to
limit which devices and users can reach it; the key is still required.

## Client-only machine

Have the server's MCP URL (ending in `/mcp`) and a copy of its access key.
Setup can ask for the key or use an existing private file. It records the
remote connection without starting a local server.

```sh
kkt setup --mode client --url https://kkt.tail1234.ts.net/mcp \
  --key-file /private/path/server-key --clients codex,claude-code
```

Keep the key file private (mode 0600). The server owner can read the key
with `kkt key show` on a service host, or the manual container command in
[Docker](docker.md#the-access-key).

## Flags and unattended setup

Each setup question has a flag. Without a terminal, an unanswered question
is an error naming the flag to supply.

```sh
kkt setup --mode service --no-tailscale --clients codex
kkt setup --mode docker --tailscale --clients none
```

| Flag | Answer |
|---|---|
| `--mode service`, `docker` or `client` | What this machine is for |
| `--tailscale` / `--no-tailscale` | Whether to share a server over Tailscale |
| `--url URL` | A client-only machine's server MCP URL |
| `--key-file PATH` | A client-only machine's existing server key file |
| `--clients claude-code,codex,opencode,agy` | Which clients to connect; `none` skips them |
| `--yes` | Connect every detected client without asking |
| `--ts-hostname NAME` | Docker Tailscale node name to request (default `kkt`) |
| `--ts-authkey-file PATH` | Docker Tailscale auth key file instead of an interactive login link |

`--yes` does not choose the server mode, Tailscale setting, or a remote
URL/key for you. A Docker Tailscale login may still need a person to open
the link. For unattended Docker login, pass `--ts-authkey-file` with a
private file holding an auth key; keep that file out of shared logs and
repositories.

## Managing the saved setup

```sh
kkt status
kkt backup ~/kkt-backups
kkt upgrade
kkt uninstall
```

`status` reports the selected setup. `backup` acts on that server, exporting
Docker backups into the host folder you name. In Docker mode the folder
argument is optional: `kkt backup` alone leaves the backup in the managed
`docker/backups` folder. Service mode still requires a destination.

Install a new version of the program before `upgrade`: a Docker setup
backs up, updates its managed
files to that version's image and starts it; a service backs up and
reinstalls the service using the installed program. See
[backups](backup.md) for restoring an older database when rolling back.
The installed version is the upgrade target. If it is already selected,
`upgrade` reports that and makes no changes. It refuses a lower version or
an image tag it cannot compare unless you pass `--allow-downgrade`.
That flag only overrides the version check: to roll back across database
schema changes, restore a compatible backup while the server is stopped
(see [restoring](backup.md#restoring)). Client-only mode reports that backup
and upgrade belong on the server.

`uninstall` removes the managed service or containers and the setup record,
keeping data, keys and client settings. It leaves the `kkt` executable
installed. See the [README](../README.md#uninstall) for removing client
entries and saved data too.

The setup record is `~/.config/kkt/setup.toml` (`$XDG_CONFIG_HOME/kkt/`
if set). It records the mode, connection URL, key path, and the Docker
folder when applicable. The record is written only after setup succeeds;
failure leaves the previous record in place. Follow the reported fixes and rerun setup.

`kkt connect` and `kkt doctor` default to this record's URL and key path;
`--url` and `--key-file` override them. See [clients](clients.md).
`kkt serve` refuses on Docker and client-only machines, pointing to the
recorded server. With a service setup it refuses while the service is
running, and allows a terminal server when the service is stopped.
`kkt serve --ignore-setup` explicitly overrides the guard. With no setup
record, `serve` works as before; `backup --db ... --config ...` also keeps
its manual behavior.
