# Connecting clients

Run `kkt setup` to install a server or connect this machine to one elsewhere.
Setup offers detected clients. To add another client later:

```sh
kkt connect claude-code --write
kkt connect codex --write
kkt connect agy --write
kkt connect opencode --write
kkt connect muse-code --write
```

Run without `--write` to see the config or command and its destination first.
When setup has been run, the URL and key default to its saved connection.
Otherwise the URL comes from the serve config's `bind`. `--url https://your-host/mcp`
overrides it, for example when forwarding over a tailnet. `--config PATH`
selects a serve config; `--key-file PATH` overrides its key file. Without a setup record or an
override, connect uses the same key as serve and creates a missing default
key with mode 0600. A config with `no_access_key = true` adds no credentials.

Writes preserve other servers and settings. Changed files are backed up as
`.bak`, then `.bak.1`, and so on; an existing backup is never overwritten.
Repeating the same command does not change the file or make another backup.
Malformed files are refused before any change. Config files and backups
written by kkt are readable only by their owner. Claude Code writes its own
config through its CLI; kkt backs it up before invoking that CLI.

| Client | Destination | Credentials |
|---|---|---|
| Claude Code | `~/.claude.json`, user scope | `headersHelper` calls this `kkt` executable with the absolute key path. `claude` must be on PATH for `--write`. |
| Codex | `~/.codex/config.toml` | `http_headers_helper` calls `kkt`; existing TOML comments are retained. |
| Antigravity (`agy`) | `~/.gemini/config/mcp_config.json` | Copies the key into `headers`. An existing legacy `~/.gemini/antigravity/mcp_config.json` is used instead if the newer path is absent. |
| OpenCode | `~/.config/opencode/opencode.json` | `Authorization` uses `{file:<absolute key path>}`; OpenCode trims surrounding whitespace. OAuth is disabled for this static key. |
| Muse Code (`muse-code`) | `~/.config/muse/settings.json` (`$XDG_CONFIG_HOME/muse/settings.json` when set) | Copies the key into `headers`. The entry is `transport: streamable_http` with `mode: optional`, so a stopped kkt server does not abort Muse runs. A file created or extended by kkt carries `schema_version: 1`, which Muse Code requires. |

Antigravity and Muse Code require the key in their config files: neither
runs a header helper for a static header. Connect warns about this, sets
mode 0600, and redacts the key in previews. The other clients refer to the
key file, so they pick up its current contents without copying it or
requiring a shell environment variable.

`kkt key header` prints `Authorization: Bearer <key>` for helper use.
`kkt key header --json` prints the JSON header map Claude Code and Codex
require. Both accept `--key-file PATH`; neither creates a key. They print
the key intentionally, so use them as helpers rather than in shared logs.

## Check a connection

```sh
kkt doctor
```

Doctor checks the key file and mode, server
reachability, Host acceptance, and MCP authentication. With a Docker setup
it also checks the container state; with a service setup (or no setup
record) it checks this machine's service. Client-only mode skips local
server state. Each problem includes a fix. It accepts the same `--config`, `--url` and
`--key-file` overrides as connect. It does not create keys, register an
agent, or change server data. Its short MCP session is closed after the
probe. HTTP redirects are not followed with the key.
When checking a local service, an absent service is optional: the server
may be running in a terminal.
An installed service that has stopped is reported with a start command.

The MCP initialize response exposes the running version. It does not expose
the commit; the web page's server runs show that provenance.

The config formats follow the vendors' documentation:
[Claude Code MCP](https://code.claude.com/docs/en/mcp),
[OpenAI Docs for Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli),
[Antigravity MCP](https://antigravity.google/docs/mcp),
[OpenCode configuration](https://opencode.ai/docs/config/), and, for
Muse Code, [Muse Code](https://developer.meta.com/ai/products/muse-code/) with its
`mcpServers` settings example in
[Wearables Documentation MCP Server](https://wearables.developer.meta.com/docs/develop/webapps/wearables-documentation-mcp/).
OpenCode's file substitution trims contents in its
[configuration loader](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/config/variable.ts).
