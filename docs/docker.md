# Docker by hand

For guided installation, run `kkt setup --mode docker` (see [setup](setup.md)).
This page covers Compose files you manage yourself; `kkt upgrade` manages
only the Docker folder recorded by setup.

The image runs `kkt serve --bind 0.0.0.0:7420 --allow-any-bind`. A container
has to listen on its own network interface, so it uses `--allow-any-bind`;
kkt accepts that only with an access key. Keep the port on the host's
loopback (`127.0.0.1:7420:7420`), as `compose.yaml` does, or behind the
Tailscale container.

## Setup

Choose a [published release](https://github.com/sam-mfb/kashkeshet/releases)
and download its files into a new folder. The commands below use 0.4.2;
for another release, change `KKT_VERSION` (without the `v`). Keep the files
and image on the same release; the backup commands in this guide need 0.4.0
or later.

```sh
mkdir kkt && cd kkt
KKT_VERSION=0.4.2
curl -fsSLO "https://raw.githubusercontent.com/sam-mfb/kashkeshet/v${KKT_VERSION}/compose.yaml"
curl -fsSL "https://raw.githubusercontent.com/sam-mfb/kashkeshet/v${KKT_VERSION}/.env.example" -o .env
mkdir backups
```

Set `KKT_VERSION` in `.env` to the same version. There is no default.
Before the first start, set the backups folder's ownership on Linux as
explained below, then run:

```sh
docker compose up -d
```

To publish kkt on a port other than 7420 of the host's `127.0.0.1`, add
`KKT_PORT=<port>` to `.env`.

Everything kkt keeps is under `/data`, in the `kkt-data` volume: the
database (`/data/.local/share/kashkeshet/kashkeshet.db`), the access key
(`/data/.config/kkt/access-key`), the operator web key
(`/data/.config/kkt/web-key`, both made on first start), and the config file
(`/data/.config/kkt/config.toml`, if you make one). The `backups` folder
beside the Compose file is mounted at `/backups`.

The container runs as user 10001. On Linux that user must be able to write
to `backups`: make it before the first `docker compose up`, then run
`sudo chown 10001 backups`. Docker Desktop (Mac and Windows) needs only the
`mkdir`. If the folder is missing at start, Docker makes it owned by root,
and on Linux backups to it fail until you run the same `chown`.

## Backups

```
docker compose exec kkt kkt backup /backups
```

This leaves a folder in `backups` with the database, the key and the config
file (see [backups](backup.md)). On Linux the files belong to user 10001 and
are readable only by it, because they hold the access key; use `sudo` to
read or copy them.

## Upgrading

Change `KKT_VERSION` in `.env`, then:

```
docker compose up -d
```

Compose pulls the new image and restarts kkt. If the new version upgrades
the database, it first backs it up inside the volume, in
`/data/.local/share/kashkeshet/upgrade-backups`, and refuses to start if it
can't. Making your own backup first, as above, also puts one on the host.

## Upgrading a manual install from 0.3.0 or earlier

Those images have no `kkt backup` command. Before changing `KKT_VERSION`,
stop kkt and copy the complete data volume with the old image:

```sh
docker compose stop kkt
docker compose run --rm --no-deps --entrypoint sh kkt -c '
  mkdir -m 700 /backups/before-upgrade &&
  cp -R /data/. /backups/before-upgrade/'
```

Use a new folder name for each snapshot; `mkdir` refuses to overwrite an
existing one. This is a copy made while the server is stopped, including
its database, key and config. Keep it private. On Linux it belongs to user
10001, so use `sudo` when reading or copying it. If copying fails, leave
the old version selected and fix the problem before upgrading.

After the copy succeeds, change `KKT_VERSION` to the new release and run
`docker compose up -d`. Later backups can use `kkt backup` as above. The
snapshot keeps the original `/data` layout; its database is under
`.local/share/kashkeshet/`, not at the top level like a `kkt backup` folder.
A manual Compose install stays manually managed: `kkt upgrade` uses only
the Docker folder recorded by `kkt setup`.

### Restoring this raw snapshot

A stopped server can leave committed data in `kashkeshet.db-wal`. This raw
snapshot keeps that file too. Unlike a `kkt backup` folder, its `.db` file
alone may not contain the data. Do not use the single-file restore below
for this snapshot.

Set `KKT_VERSION` back to the old version, then restore the database and
its WAL together, if the snapshot has one. Discard the current database's
WAL and shared-memory file; never reuse them with the restored database:

```sh
docker compose stop kkt
docker compose run --rm --no-deps --entrypoint sh kkt -c '
  cd /data/.local/share/kashkeshet &&
  rm -f kashkeshet.db-wal kashkeshet.db-shm &&
  cp /backups/before-upgrade/.local/share/kashkeshet/kashkeshet.db . &&
  if [ -f /backups/before-upgrade/.local/share/kashkeshet/kashkeshet.db-wal ]; then
    cp /backups/before-upgrade/.local/share/kashkeshet/kashkeshet.db-wal .
  fi'
docker compose up -d
```

Do not start the server if copying fails; fix the error and repeat the
restore first. The snapshot's key and config are under `.config/kkt/` if
you also need to restore them.

## Rolling back a `kkt backup` copy

Set `KKT_VERSION` back to the old version, then put back a database made by
that version or before: a backup made before the upgrade has the database
as the old version left it. Remove the database's `-wal` and `-shm` files
too; they belong to the database being replaced. For a backup in `backups`
named `kkt-20261028T131500.123Z-v0.4.0`:

```
docker compose stop kkt
docker compose run --rm --no-deps --entrypoint sh kkt -c '
  cd /data/.local/share/kashkeshet &&
  rm -f kashkeshet.db-wal kashkeshet.db-shm &&
  cp /backups/kkt-20261028T131500.123Z-v0.4.0/kashkeshet.db kashkeshet.db'
docker compose up -d
```

For a backup made before an upgrade, copy from
`upgrade-backups/<name>/kashkeshet.db` instead; that folder is in the same
directory. With Tailscale, set `COMPOSE_FILE=compose.tailscale.yaml` in
`.env` (or add `-f compose.tailscale.yaml` after `docker compose` in each
command), as for every other command here.

## Other host names

kkt answers only to `localhost`, `127.0.0.1` and `::1`, plus its tailnet
name with Tailscale. To let agents in other containers on this machine reach
it as `host.docker.internal`, set `KKT_ALLOW_HOSTS` in `.env` (names
separated by commas) and run `docker compose up -d`:

```sh
KKT_ALLOW_HOSTS=host.docker.internal
```

Both Compose files pass it to kkt as `--allow-host`. Use this, not
`allow_hosts` in the config file in the volume: with Tailscale, the Compose
file's own `--allow-host` replaces the config file's list.

## The access key

The container keeps two different keys in its persistent `/data` volume.
The access key is for agents only; read the operator web key to log in:

```sh
docker compose exec kkt kkt key show --web
```

For `compose.tailscale.yaml`, add `-f compose.tailscale.yaml` to these
commands. `kkt setup --mode docker` saves host copies as
`~/.config/kkt/docker-access-key` and `~/.config/kkt/docker-web-key`, both
mode 0600. On that host, `kkt key show --web` reads the web copy. Client
configurations receive only the access key. An agent running as the host's
user can still read both files.

Rotate the server's web key inside its container, then restart it:

```sh
docker compose exec kkt kkt key create --web --force
docker compose restart kkt
```

The restart loads the new key and ends all existing sessions. Rerun
`kkt setup --mode docker` with the same settings to refresh the host copies.
Changing the host copy alone does not change the container's key.

Backups include `access-key` and `web-key`, each with mode 0600. Restore
both into `/data/.config/kkt/` beside the config file before restarting;
the database goes back to its original data path. See the backup's
`about.txt` for the source paths.

kkt never prints the key in its logs. To read it:

```
docker compose exec kkt kkt key show
```

Save it to its own file, and give that file to `kkt connect` with
`--key-file`. Without `--key-file`, `kkt connect` uses (and creates if
missing) the key of a kkt installed on this machine, which is a different
key.

```
mkdir -p ~/.config/kkt
(umask 077; docker compose exec -T kkt kkt key show > ~/.config/kkt/docker-access-key)
kkt connect claude-code --url http://127.0.0.1:7420/mcp --key-file ~/.config/kkt/docker-access-key --write
```

## With Tailscale

In the folder from **Setup**, download the Tailscale files from the same
release (`KKT_VERSION` is the version chosen above):

```sh
mkdir -p tailscale
curl -fsSLO "https://raw.githubusercontent.com/sam-mfb/kashkeshet/v${KKT_VERSION}/compose.tailscale.yaml"
curl -fsSL "https://raw.githubusercontent.com/sam-mfb/kashkeshet/v${KKT_VERSION}/tailscale/serve.json" -o tailscale/serve.json
```

Set the values in the table below, including
`COMPOSE_FILE=compose.tailscale.yaml` in `.env`, then run `docker compose up -d`.
Every later Compose command uses that file as well.

`compose.tailscale.yaml` runs the `tailscale/tailscale` image as a separate
container beside kkt, on the same Compose network. kkt is published on the
host's loopback only (`127.0.0.1:7420`), as in `compose.yaml`. Tailscale Serve (`tailscale/serve.json`) takes HTTPS on port 443
and proxies it to `http://kkt:7420`. kkt listens on all addresses inside its
own container, as in the plain image, so it needs the access key.

Who can reach kkt over plain HTTP:

- **The tailnet:** no one. Only the Tailscale container is on the tailnet, and
  it answers HTTPS on port 443. Connections to port 7420 on the tailnet
  address reach that container, where nothing listens.
- **The Tailscale container:** it talks to kkt over plain HTTP on the Compose
  network. That traffic does not leave the host.
- **The Docker host:** programs on the host use `http://127.0.0.1:7420`, like
  a normal local install. On Linux they can also reach the container's
  internal address (for example `172.18.0.3:7420`) directly. On Docker
  Desktop (Mac and Windows) only the loopback port works.
- **Other machines on the local network:** no one. The port is published to
  the host's loopback only.

The access key guards kkt in every case. To limit who on your tailnet can
connect at all, use Tailscale's access controls.

Either container can be restarted alone.

If the Tailscale machine's key expires, or it is logged out, it drops off the
tailnet while kkt keeps running on the host. With `kkt setup`, `kkt status`
and `kkt doctor` say so, and running `kkt setup` again logs it in (see
[Setup](setup.md#when-the-machine-needs-to-log-in-again)). By hand, restart
the Tailscale container and use the link in its log:
`cd '<folder>' && docker compose restart tailscale && docker compose logs tailscale`.

If a machine named `TS_HOSTNAME` is already on your tailnet, Tailscale names
the new one `<name>-1`, and kkt, which is told the name you set, refuses
requests to the new name. Remove the old machine in the Tailscale admin
console before starting, or set `TS_HOSTNAME` to the name the node got.

Values in `.env`:

| Name | Meaning |
|---|---|
| `COMPOSE_FILE` | `compose.tailscale.yaml`, so that every `docker compose` command uses it. Without it, commands such as `up -d`, `exec` and `down` use `compose.yaml`, which has no Tailscale container and no tailnet name for kkt |
| `TS_AUTHKEY` | Optional: a Tailscale auth key. Without one, `docker compose logs tailscale` shows a link to log in with |
| `TS_HOSTNAME` | The machine name kkt gets on the tailnet (`kkt`) |
| `TS_TAILNET` | Your tailnet's DNS name, such as `tail1234.ts.net` |

kkt must be told the name it is served under, or it refuses the requests
(see [security](security.md)). The Compose file builds it as
`${TS_HOSTNAME}.${TS_TAILNET}` and passes it as `--allow-host`, so there is
nothing to edit. The same name appears in `serve.json` as
`${TS_CERT_DOMAIN}`, which the Tailscale container fills in.

The Tailscale container uses userspace networking, so it needs no
`/dev/net/tun` and no extra capabilities. Its state is in the
`tailscale-state` volume, so it keeps its identity across restarts. HTTPS
certificates must be enabled for your tailnet in the Tailscale admin
console.

Tailscale machine keys expire after 180 days by default (your tailnet's
policy can differ). For a server that should stay connected, open the
admin console's **Machines** page, find the kkt machine, and choose
**Disable key expiry** from its menu. Otherwise it needs reauthentication
when the key expires; see [Tailscale's key expiry guide](https://tailscale.com/docs/features/access-control/key-expiry).

HTTPS certificates put the machine's full DNS name, including its machine
and tailnet names, in public Certificate Transparency logs. Choose names
you are comfortable making public; see [Tailscale's HTTPS guide](https://tailscale.com/docs/how-to/set-up-https-certificates).

## Building the image yourself

```
docker build -t kkt .
```

Build from a git checkout, so the server records its commit.
