# Phase 57: EggPool configuration and CLI

Status: completed.

## Objective

Add one optional, validated EggPool endpoint to Gregg's existing configuration and expose nested `eggpool add`, `eggpool list`, and `eggpool remove` commands through the current atomic `ConfigStore`.

This phase ends when:

- old configuration files without EggPool data load unchanged;
- one EggPool entry can be added, listed, replaced, and removed deterministically;
- host, port, scheme, display name, and optional API-key environment-variable name are validated;
- no resolved credential value is persisted or emitted;
- the TUI can later determine whether an EggPool pane exists from `Config::eggpool.is_some()`.

HTTP requests, polling, state, key behavior, and rendering are Phases 58 and 59.

## Dependencies and execution position

Depends on the completed client configuration/CLI baseline and Phase 56 roadmap.

Must complete before:

- Phase 58 constructs authenticated EggPool requests;
- Phase 59 derives the available pane set;
- Phase 60 performs runtime integration and documentation closure.

## Governing invariants

1. Existing `systems` entries and top-level `add`, `list`, and `remove` commands remain unchanged.
2. Configuration schema version remains `1`.
3. The new EggPool field is optional and defaults to absent.
4. Exactly one EggPool endpoint is supported.
5. The EggPool default port is `11300`, independent from greggd's `11310` default.
6. The only supported schemes are `http` and `https`.
7. Configuration stores an API-key environment-variable name, never a resolved key.
8. All writes continue through `ConfigStore` locking, validation, temporary-file serialization, and atomic replacement.
9. Unknown TOML fields remain rejected.
10. No network request is performed by add/list/remove.
11. No generic endpoint abstraction or datasource registry is introduced.
12. No new dependency is required.

## Scope

### In scope

- `EggpoolEntry` and `EggpoolScheme` configuration types;
- EggPool endpoint parsing/normalization;
- optional `Config::eggpool` field;
- configuration validation and violation display;
- nested Clap command group;
- add/list/remove implementation;
- `--name`, `--https`, `--api-key-env`, and `--replace` handling;
- human and JSON list output;
- singleton conflict behavior;
- example configuration and focused CLI/config tests.

### Out of scope

- HTTP requests or authentication headers;
- checking whether EggPool is reachable;
- validating the secret value in the environment;
- multiple EggPool endpoints;
- credentials stored inline, in files, or in a keyring;
- changing existing greggd endpoint syntax;
- URL paths, query strings, usernames, passwords, fragments, or arbitrary base URLs;
- custom TLS roots, certificate bypasses, redirects, proxies, or cookies;
- TUI pane state or rendering;
- changes to EggPool, `greggd`, or `gregg-protocol`;
- new CI or release machinery.

## Workstream A: define the optional configuration model

Add a client-owned entry with a narrow shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EggpoolEntry {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub scheme: EggpoolScheme,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EggpoolScheme {
    Http,
    Https,
}
```

Add to `Config`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub eggpool: Option<EggpoolEntry>,
```

`Config::default()` sets `eggpool = None`.

Do not add a second top-level default port. EggPool's default is a parser/command constant because only one optional entry exists and the requested CLI convention does not require a configurable EggPool default.

Recommended constants:

```rust
pub const DEFAULT_EGGPOOL_PORT: u16 = 11300;
pub const MAX_EGGPOOL_NAME_LEN: usize = 128;
pub const MAX_ENV_NAME_LEN: usize = 128;
```

### Workstream A acceptance criteria

- [ ] Existing TOML without `[eggpool]` deserializes to `None`.
- [ ] Serialization omits the EggPool table when absent.
- [ ] A populated entry round-trips deterministically.
- [ ] `config_version` remains `1`.
- [ ] No secret-value field exists.

## Workstream B: implement an EggPool-specific endpoint parser

Create a small module such as:

```text
crates/gregg/src/eggpool_endpoint.rs
```

The accepted positional syntax remains similar to greggd:

```text
HOST
HOST:PORT
[IPv6]:PORT
bare IPv6
```

Parsing requirements:

- trim surrounding whitespace using the existing command convention;
- normalize DNS host case consistently with existing endpoint behavior;
- normalize IP literals through `IpAddr` where possible;
- reject empty host;
- reject port zero, nonnumeric port, and overflow;
- reject URL schemes in the positional argument;
- reject paths, query strings, fragments, credentials, and `@`;
- support bare IPv6 with the default port;
- require brackets for explicit IPv6 port syntax;
- track whether the port was explicit so remove can distinguish host-wide from exact matching.

The scheme comes from a flag:

```text
no flag   -> http
--https   -> https
```

Do not accept arbitrary scheme strings or a complete URL. This keeps the command consistent with existing Gregg endpoint management and avoids path/base-URL ambiguity.

Provide one canonical address formatter that brackets IPv6 and may include the scheme for EggPool list output:

```text
http://eggpool.local:11300
https://[2001:db8::1]:443
```

Do not alter `endpoint.rs` unless a tiny private parsing helper can be shared without changing greggd semantics. Prefer duplication of a few narrow validation steps over an over-generalized endpoint framework.

### Workstream B acceptance criteria

- [ ] Default port is `11300`.
- [ ] HTTP and HTTPS are represented separately from the host parser.
- [ ] DNS, IPv4, bare IPv6, and bracketed IPv6 cases are tested.
- [ ] Schemes, paths, credentials, query strings, and invalid ports are rejected.
- [ ] Existing greggd endpoint parsing and tests are untouched.

## Workstream C: validate display name and environment-variable reference

Display-name rules should match existing endpoint names:

```text
nonempty after trim
maximum 128 bytes/characters according to the existing name policy
no silent truncation
```

For `api_key_env`, use a conservative cross-platform environment-name rule:

```text
[A-Za-z_][A-Za-z0-9_]*
```

Additional requirements:

- trim is not accepted silently; persist the exact validated name;
- maximum length is bounded;
- empty string is invalid;
- values such as `SERVER_API_KEY`, `EGGPOOL_GREGG_API_KEY`, and `_LOCAL_KEY` pass;
- values containing `=`, whitespace, hyphen, shell expansion syntax, path separators, or NUL/control characters fail;
- validation checks only the variable name, not whether the variable currently exists.

Add focused `ConfigViolation` variants rather than returning unstructured strings. Suggested variants:

```rust
InvalidEggpoolHost { host: String }
InvalidEggpoolPort { port: u16 }
InvalidEggpoolName { reason: String }
InvalidEggpoolApiKeyEnv { value: String, reason: String }
DuplicateEndpointId { ... } // reuse only if the existing meaning is general enough
```

A UUID-v4 format check may remain consistent with current system entries. Do not add cryptographic or ownership semantics to the ID.

### Workstream C acceptance criteria

- [ ] Valid environment-variable names are accepted cross-platform.
- [ ] Secret-shaped values are not mistaken for environment-variable names when they contain invalid punctuation.
- [ ] Validation never reads the environment.
- [ ] Violations are specific enough for CLI error output.

## Workstream D: add the nested Clap command surface

Extend the command enum with one group:

```rust
Eggpool {
    #[command(subcommand)]
    command: EggpoolCommand,
}
```

Subcommands:

```text
gregg eggpool add ENDPOINT [--name NAME] [--https]
                      [--api-key-env ENV] [--replace]
gregg eggpool list [--json]
gregg eggpool remove ENDPOINT
```

Help text must state:

- default port is `11300`;
- one EggPool endpoint is supported;
- `--replace` replaces the current entry;
- `--api-key-env` stores only an environment-variable name;
- `--https` changes the scheme;
- dashboard/statistics route availability is checked later by the TUI, not during add.

Do not add aliases that collide with existing top-level commands. Do not add an interactive prompt.

### Workstream D acceptance criteria

- [ ] Existing top-level command parsing remains unchanged.
- [ ] Every nested command has stable examples/help.
- [ ] `--config` remains global and works with nested commands.
- [ ] No network or environment lookup occurs during parsing.

## Workstream E: implement add and replace semantics

`eggpool add` behavior:

1. parse endpoint with default `11300`;
2. validate optional display name and environment-variable name;
3. create an `EggpoolEntry` with a new UUID;
4. mutate through `ConfigStore`;
5. if `config.eggpool` is absent, insert it;
6. if present and `--replace` is false, return a validation/operation error explaining that only one EggPool is supported;
7. if present and `--replace` is true, replace it atomically;
8. print a concise diagnostic to stderr consistent with existing commands.

Replacement is unconditional once explicitly requested. Do not require the new address to match the old one.

Do not resolve `api_key_env` or test connectivity.

Required tests:

- add into empty config;
- add with default port/scheme;
- add with explicit port;
- add HTTPS;
- add with name;
- add with env reference;
- reject second add without replace;
- replace with a different endpoint;
- failed validation leaves the original config unchanged;
- write/reload round trip.

### Workstream E acceptance criteria

- [ ] Singleton behavior is explicit and deterministic.
- [ ] Replacement cannot leave a partial or invalid config.
- [ ] Existing systems remain byte/semantically unchanged during EggPool mutation.
- [ ] No resolved key appears in config or command output.

## Workstream F: implement list output with redaction guarantees

Human output should print zero or one line. Suggested shape:

```text
Main EggPool  http://eggpool.local:11300  auth-env=EGGPOOL_GREGG_API_KEY
```

When no EggPool exists, print nothing, matching existing empty list behavior.

JSON output should serialize the configured entry or a stable collection shape chosen to mirror current list scripting conventions. Preferred shape for command-family consistency:

```json
[]
```

or:

```json
[
  {
    "id": "...",
    "host": "eggpool.local",
    "port": 11300,
    "scheme": "http",
    "name": "Main EggPool",
    "api_key_env": "EGGPOOL_GREGG_API_KEY"
  }
]
```

Use an array even though cardinality is at most one so `list --json` retains the existing conceptual shape and future scripts do not need null/object special cases.

Redaction tests must use a synthetic secret value in the environment and prove that neither human nor JSON output contains it.

Do not print whether the variable is currently set; that would mix configuration listing with runtime secret probing.

### Workstream F acceptance criteria

- [ ] Empty list behavior is deterministic.
- [ ] Human output contains address and environment-variable name only.
- [ ] JSON output contains no resolved credential.
- [ ] A regression test searches output for a synthetic secret value.

## Workstream G: implement remove semantics

`eggpool remove ENDPOINT` mirrors existing host versus host:port behavior:

- host only removes the configured EggPool when the normalized host matches, regardless of port;
- host:port removes only when both normalized host and port match;
- scheme is not part of the positional remove match because the existing convention accepts `HOST[:PORT]`;
- no match prints a concise diagnostic and returns success, matching current `remove` behavior;
- a match sets `config.eggpool = None` through `ConfigStore`.

Required tests:

- remove by host;
- remove by exact host/port;
- mismatched host;
- mismatched port;
- IPv6 matching;
- removal preserves all systems and global settings;
- repeated remove remains nonfatal.

### Workstream G acceptance criteria

- [ ] Remove matching is consistent with existing endpoint conventions.
- [ ] Removal is atomic.
- [ ] No unrelated configuration field changes.

## Workstream H: example and migration documentation

Update `crates/gregg/config.example.toml` with a commented optional block:

```toml
# Optional EggPool statistics source. Omit this table to disable the pane.
# [eggpool]
# id = "..."
# host = "eggpool.local"
# port = 11300
# scheme = "http"
# name = "Main EggPool"
# api_key_env = "EGGPOOL_GREGG_API_KEY"
```

Document only the configuration format in this phase. Full TUI/API/auth behavior belongs to Phase 60.

Do not rewrite old config files, add a migration command, or bump `config_version`. Serde defaulting is the migration mechanism.

### Workstream H acceptance criteria

- [ ] Example config is valid when uncommented with a valid UUID.
- [ ] Omission is documented as disabling the pane.
- [ ] No migration machinery or version bump is added.

## Expected files

Likely files:

```text
crates/gregg/src/config.rs
crates/gregg/src/cli.rs
crates/gregg/src/eggpool_endpoint.rs
crates/gregg/src/main.rs            # module declaration only if needed
crates/gregg/config.example.toml
crates/gregg/src tests adjacent to the modules
```

Do not touch polling, scheduler, state, event, or UI modules in this phase except compile-only scaffolding that carries `Config::eggpool` without behavior.

## Implementation sequence

1. Add failing old-config/default/round-trip tests.
2. Define `EggpoolScheme`, `EggpoolEntry`, and optional config field.
3. Add validation variants and focused tests.
4. Implement the EggPool endpoint parser and formatter.
5. Add nested Clap enums/help tests.
6. Implement add and replace through `ConfigStore`.
7. Implement list and redaction tests.
8. Implement remove and preservation tests.
9. Update example TOML.
10. Run focused checks and inspect the diff for network/UI/generalization scope.

## Required verification

Use focused commands:

```text
cargo fmt --all -- --check
cargo test -p gregg config --all-features
cargo test -p gregg cli --all-features
cargo test -p gregg eggpool_endpoint --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

If test-name filters do not align with module names:

```text
cargo test -p gregg --all-targets --all-features
```

Do not add a new script or workflow.

## Phase acceptance criteria

Phase 57 is complete only when:

- [ ] Existing configuration files without EggPool data load unchanged.
- [ ] `Config::default().eggpool` is `None`.
- [ ] Config schema version remains `1`.
- [ ] One optional entry stores host, port, scheme, name, and API-key environment-variable name.
- [ ] Default EggPool address semantics are HTTP port `11300`.
- [ ] Host/port parsing correctly supports DNS, IPv4, and IPv6 and rejects schemes/paths/credentials/invalid ports.
- [ ] Display names and environment-variable names are bounded and validated.
- [ ] `gregg eggpool add` inserts one entry atomically.
- [ ] A second add fails unless `--replace` is explicit.
- [ ] `gregg eggpool list` and `--json` are deterministic and never emit a resolved key.
- [ ] `gregg eggpool remove` supports host-only and exact host:port matching.
- [ ] EggPool mutations preserve all systems/global settings.
- [ ] Example TOML documents the optional block.
- [ ] No connectivity check, environment lookup during mutation, HTTP code, TUI behavior, new dependency, or new infrastructure was added.

## Handoff guidance for a smaller implementation model

- Copy the behavioral shape of existing add/list/remove, not the greggd port/scheme assumptions.
- Keep one `Option<EggpoolEntry>`; do not use a vector.
- Store `api_key_env`, never `api_key`.
- Prefer a small dedicated parser over changing `EndpointSpec` into a generic URL abstraction.
- Write redaction tests before list output implementation.
- Stop if implementation starts introducing endpoint traits, datasource registries, migration frameworks, or secret-management systems.
