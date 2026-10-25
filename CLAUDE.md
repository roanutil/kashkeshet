# Working on kashkeshet

These are the general coding principles turned into rules for this codebase.
The README explains what the server does and where the code lives; read it
first.

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
| `web/` | The human's read-only pages, and logging in to them | Change anything except sign-in sessions, through `admission` |
| `hosts.rs` | Which `Host` names the server answers to, for `/mcp` and the web page | Touch storage, rules, or HTTP library types |
| `settings/` | Gathering a server's settings from flags, the config file and defaults; default paths | Read files or environment variables itself; check limits (that is `config.rs`) |
| `config.rs` | Every limit, fixed setting and default, and the startup checks | Hold logic beyond checking settings |
| `provenance/` | What produced a server run | |
| `app.rs` | Assembling the parts | Hold business logic |

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

**Library types.** Third-party types do not appear in our interfaces. Wrap them
in our own types inside the part that uses the library: `events::Subscription`
over tokio's broadcast receiver, `shutdown::Shutdown` over tokio-util's
cancellation token, `store` over rusqlite. rmcp stays in `mcp.rs` and
`app.rs`; askama stays in `web/`; axum stays in `web/`, `app.rs`, and
`main.rs`, which serves the router; toml stays in `settings/`; clap stays in
`main.rs`.

**Settings and defaults.** Every limit or fixed value lives in `config.rs`,
with its default written out. Settings agents or operators can change are
fields of `Limits`; others are constants. Add a check to `Limits::check` for
any new limit, and add it to `provenance::settings_text` (the build will fail
until you do). When a library type is configured, set every option we rely on
by name; do not use `::default()` and hope. Bad settings stop the server with
an error naming the setting; never correct them quietly.

**What agents see.** The MCP tools are a contract with every agent that uses
the server. Do not rename or remove tools, parameters, error codes, or output
fields; add new ones instead. Write limits in tool and parameter descriptions as
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

## Working with other agents

Stay inside your own working directory. Share work only through git: push a
branch and open a pull request with `gh`. Do not read or copy another agent's
files. Pull requests target the integration branch the coordinator names. Only
the repository owner merges into `main`.
