# Docker

The image runs `kkt serve --bind 0.0.0.0:7420 --allow-any-bind`. A container
has to listen on its own network interface, so it uses `--allow-any-bind`;
kkt accepts that only with an access key. Keep the port on the host's
loopback (`127.0.0.1:7420:7420`), as `compose.yaml` does, or behind the
Tailscale container.

## One volume

Everything kkt keeps is under `/data`: the database, the access key
(`/data/.config/kkt/access-key`, made on first start) and the config file
(`/data/.config/kkt/config.toml`). Upgrading means pulling a new image and
starting the container again; the volume stays.

```
docker compose pull
docker compose up -d
```

## The access key

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

`compose.tailscale.yaml` runs the `tailscale/tailscale` image as a separate
container, and starts kkt with `network_mode: service:tailscale`, so both
share one network. kkt listens on `127.0.0.1:7420` inside it. Nothing is
published on the host, and the port is not open on the tailnet address:
Tailscale Serve (`tailscale/serve.json`) is the only way in, over HTTPS on
port 443.

Values in `.env`:

| Name | Meaning |
|---|---|
| `TS_AUTHKEY` | A Tailscale auth key |
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

## Building the image yourself

```
docker build -t kkt .
```

Build from a git checkout, so the server records its commit.
