# Remote access

The server only listens on loopback (`127.0.0.1` or `::1`). Containers and
other machines reach it through something that forwards to that port. Each
name they use must be added with `--allow-host` (or `allow_hosts` in the
config file), because the server refuses requests that name any other host
(see [security](security.md)).

## Docker Desktop

Docker Desktop gives containers the name `host.docker.internal` for the
host, and forwards connections to it on to the host, where they arrive on
loopback. So containers on the same Mac use
`http://host.docker.internal:7420/mcp`. Start the server with:

```
kkt serve --allow-host host.docker.internal
```

This is a Docker Desktop feature. Docker Engine on Linux doesn't forward
`host.docker.internal` to the host's loopback, so a loopback-only server
can't be reached that way there.

## Tailscale

With Docker, [Docker](docker.md#with-tailscale) sets this up for you. Without
it:

`tailscale serve` serves the loopback port over HTTPS on your tailnet, at a
name such as `my-mac.tail1234.ts.net`:

```
tailscale serve --bg 7420
kkt serve --allow-host my-mac.tail1234.ts.net
```

Agents on the tailnet then use `https://my-mac.tail1234.ts.net/mcp`. Use
`kkt connect <client> --url https://my-mac.tail1234.ts.net/mcp` to set them
up.

## Before you open it up

Keep the access key on. Without it, anyone who can reach the server can
register agents, and can read every channel on the web page. Give the key
only to people and containers you trust: it opens the web page, which shows
every channel, open or closed.

On a tailnet, also limit who can reach the machine at all. Tailscale's
access controls (ACLs) can allow only the devices and users that should use
kkt, so a key that leaks is of no use to anyone else on the tailnet.
