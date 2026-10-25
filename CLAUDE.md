# Working on kashkeshet

The README explains what kkt is and how to use it. This file is for working
on the code: the design principles, where code lives, the rules that turn
the general coding principles into practice here, and the tests.

## Design principles

1. **Simple.** Understanding kkt, starting a server and connecting agents
   must be as simple and as hard to get wrong as possible.
2. **Not opinionated.** kkt provides mechanics: identities, channels,
   messages, and waiting for them. It does not tell developers or agents how
   to work; they find their own patterns in chat. The test for a feature:
   does it add a general mechanism, or prescribe a way of working? Only the
   first belongs in kkt.
3. **No backwards compatibility yet.** The project is young, so keeping
   existing setups working is not a concern. Prefer one clear way over
   keeping old ways alongside it.

## The parts and their boundaries

Each directory or file under `src/` owns one area. Work in one area should not
require reading another's internals.

| Part | Owns | Must not |
|---|---|---|
| `admission/` | Whether an HTTP caller is let in to `/mcp` or the web page at all, from credentials in our own type; the web page's sessions | Know about agents, agent tokens or channels; import axum |
| `rules/` | Every permission decision and every change, for both MCP and the web page | Import rusqlite or write SQL; format output; log |
| `rules/access.rs` | Who may use a channel, as the pure function `decide` | Touch storage |
| `store/` | The `Reads`/`Writes` contract and its SQLite implementation | Decide permissions; know about agents' requests |
| `format/` | Turning rule results into text or JSON within the size cap | Query storage or apply rules |
| `mcp.rs` | Tool definitions; parse parameters, call `rules`, call `format` | Touch storage or decide permissions |
| `web/` | The human's read-only pages, and logging in to them. Pages read only through web's own contract (`web/source.rs`: `Source` and `Gate`), which the server provides over `rules` and `admission` (`web/real.rs`) and the preview provides from simulated data (`web/preview.rs`, run by `examples/web_preview.rs`) | Change anything except sign-in sessions, through `admission`; read `rules` outside `web/real.rs` |
| `hosts.rs` | Which `Host` names the server answers to, for `/mcp` and the web page | Touch storage, rules, or HTTP library types |
| `service/` | Running the server as a launchd or systemd service: the service files and the commands that manage them, through its `System` contract | Touch the system except through `System`; decide server settings (`main.rs` passes them in) |
| `settings/` | Gathering a server's settings from flags, the config file and defaults; default paths | Read files or environment variables itself; check limits (that is `config.rs`) |
| `config.rs` | Every limit, fixed setting and default, and the startup checks | Hold logic beyond checking settings |
| `provenance/` | What produced a server run | |
| `events.rs` | New-message notifications for waiting agents and the web page | Decide who may see a message |
| `shutdown.rs` | The stop signal (Ctrl-C, SIGTERM) and the `Shutdown` that ends the server | |
| `names.rs` | Agent names, channel IDs, and the randomness behind them (`Entropy`) | |
| `clock.rs` | The current time and waiting (`Clock`), injected so tests can control them | |
| `app.rs` | Assembling the parts | Hold business logic |
| `main.rs` | The `kkt` command: its subcommands, and the files and environment they read | Hold business logic |

Outside `src/`:

- `migrations/`: the database schema, one numbered file per change.
- `agent-instructions.md`: what the server tells each agent when it
  connects. It describes how the tools work, nothing more.
- `build.rs`: records the git commit at build time (`unknown` outside a git
  checkout).
- `tests/contract/tools.json`: the tools agents see, as pinned by
  `tests/contract.rs`.

Put new code in the part that owns its area. If no part fits, add a new one
named for the area (e.g. `auth`, `search`), not for a kind of code (`types`,
`utils`, `helpers`).

## Rules

**Storage.** Only `src/store/sqlite.rs` may import rusqlite or contain SQL. If
the rules need a new question or change, add a plain, named method to `Reads`
or `Writes` (in our own terms, using our own record types) and implement it in
`sqlite.rs`. A rule that must be atomic runs as one `Db::write` closure. Schema
changes go in a new numbered file in `migrations/` that is added to the
`MIGRATIONS` list. Never edit a migration that has shipped.

**Permissions.** Every check on whether an agent may use a channel goes through
`rules::access::decide`. To an agent outside a closed channel, the channel does
not exist: the answer must be `not_found`, exactly as for a missing ID.

**Time and randomness.** Get the current time only from the injected `Clock`,
and wait only through `Clock::sleep_until`. Get randomness only from the
injected `Entropy`. Never call `Utc::now`, `Instant::now`, tokio's timers, or
`rand` outside `clock.rs` and `names.rs`.

**Injection.** A part receives what it depends on when it is built, through
`rules::Deps` or `app::Parts`. Do not create a dependency inside the part that
uses it. Do not add a trait in front of `Rules`; there is one implementation.
(`web::Source` is not one: it is the web page's contract for its own needs,
with two implementations, the server's and the preview's.)

**Library types.** Third-party types do not appear in our interfaces. Wrap them
in our own types inside the part that uses the library: `events::Subscription`
over tokio's broadcast receiver, `shutdown::Shutdown` over tokio-util's
cancellation token, `store` over rusqlite. rmcp stays in `mcp.rs` and
`app.rs`; askama stays in `web/`; axum stays in `web/`, `app.rs`, `main.rs`
and `examples/web_preview.rs`, which serve the router; toml stays in
`settings/`; clap stays in `main.rs` and `examples/`.

**Settings and defaults.** Every limit or fixed value lives in `config.rs`,
with its default written out. Settings agents or operators can change are
fields of `Limits`; others are constants. Add a check to `Limits::check` for
any new limit, and add it to `provenance::settings_text` (the build will fail
until you do). When a library type is configured, set every option we rely on
by name; do not use `::default()` and hope. Bad settings stop the server with
an error naming the setting; never correct them quietly.

**What agents see.** The MCP tools are a contract with every agent that uses
the server. Do not rename or remove tools, parameters, error codes, or output
fields; add new ones instead. While design principle 3 holds, a rename or
removal that makes kkt simpler is allowed: say so plainly in the pull
request. Write limits in tool and parameter descriptions as
placeholders such as `{max_limit}` (see `mcp::describe`), never as numbers.
`tests/contract.rs` pins the tool list. After an intended change, run
`UPDATE_CONTRACT=1 cargo test --test contract` and review the diff to
`tests/contract/tools.json`. Changes to `agent-instructions.md` also change
what agents are told; keep them in step with the tools.

**Public surface.** Only the modules the binary and the integration tests use
are public (`lib.rs`). Make a module or item public only when something outside
the crate needs it.

**Errors.** Rules return `RuleError` with one of the fixed codes in
`rules/error.rs`. Database failures become `internal`, which agents should never
see in normal use. Never discard an error: return it, or log it where it is
handled. Logging happens only at the edges (`main.rs`, `mcp.rs`, `web/`), never
in `rules/`, `store/` or `format/`. Do not use an error result as a branch for
ordinary decisions; decide first (as `access::decide` does), then act.
`unwrap` and `expect` are for tests only.

**Provenance.** New stored rows that record activity (like agents and messages)
carry the `run_id` of the server run that created them. If the settings or
inputs that shape a run change, they must show up in `provenance::ServerRun`.

## Tests

`rust-toolchain.toml` pins the Rust version, with rustfmt and Clippy; rustup
installs it on the first Rust command in the repository. CI runs the three
checks below on Ubuntu and macOS for every pull request and push to `next`
and `main`, with `--locked`, and a pull request merges only when they pass.

- Run `cargo test`, `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` before every commit. All three must be clean.
- A bug fix comes with a test that fails without the fix.
- So does a change that adds robustness (retries, refunds, fallbacks) or claims
  better performance.
- Rule tests go in `src/rules/tests.rs` and use the test clock and entropy
  (`FixedClock`, `SeqEntropy`); advance `FixedClock` instead of sleeping in real
  time. Store tests for SQL with logic in it go in `src/store/tests.rs`.
  End-to-end tests in `tests/` start a server with `tests/common`.
- A schema change needs a test that an existing database migrates (see
  `tests/migrations.rs`).
- Tests that run the `kkt` binary give it its own home directory (`HOME`),
  so they never read a real config file or write a real key.

Where the tests are:

- Unit tests sit beside the code they test. `src/rules/tests.rs` has one
  rule test per row of the rules table in `rules/mod.rs`, plus the
  `not_found` behavior.
- `tests/e2e.rs`: starts a server and runs several agents through it with
  `rmcp`'s client, over both the 2026-07-28 protocol and the older one.
- `tests/contract.rs`: checks the tools agents see (names, descriptions,
  input schemas) against `tests/contract/tools.json`. After an intended
  change, run `UPDATE_CONTRACT=1 cargo test --test contract` and review the
  diff to that file. A second test starts the server with other limits and
  checks that the descriptions state them.
- `tests/migrations.rs`: opens a database made by an older version and
  checks it is brought up to date with its rows kept.
- `tests/admission.rs`: the access key on `/mcp`.
- `tests/web_login.rs`: logging in to the web page.
- `tests/kkt.rs`, `tests/serve.rs`, `tests/startup.rs`, `tests/key.rs`: the
  `kkt` command, `kkt serve` with its config file and defaults, settings it
  must refuse, and `kkt key create`.
- `tests/agent.rs`: `kkt agent` revoking an agent in the database of a
  running server, and listing agents.
- `tests/service.rs`: `kkt service install --dry-run` with the real binary.
  The other service commands are tested against a fake system in
  `src/service/tests.rs`, and the plist and unit are pinned by the files in
  `src/service/golden/`.
- `tests/shutdown.rs`: on Unix, stops a real server with SIGTERM and Ctrl-C,
  including with open event streams and pending message waits.
- `tests/common/`: starts a server on a free port for the tests above.

## Working with other agents

Stay inside your own working directory. Share work only through git: push a
branch and open a pull request with `gh`. Do not read or copy another agent's
files. Pull requests target the integration branch the coordinator names. Only
the repository owner merges into `main`.
