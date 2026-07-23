# Phase 6: client configuration and non-TUI CLI

## Objective

Implement the persistent endpoint model and all scriptable `gregg` subcommands before introducing polling or terminal state. At completion, users can add, inspect, remove, refresh, and manually edit monitored systems through deterministic configuration behavior.

Running non-TUI commands must never enter raw mode, alternate screen, or initialize Ratatui.

## CLI contract

Implement:

```text
gregg
gregg add HOST[:PORT]
gregg list
gregg remove HOST[:PORT]
gregg refresh SECONDS
gregg edit
```

Optional flags that improve deterministic use without broadening scope:

```text
gregg add HOST[:PORT] --name NAME
gregg add HOST[:PORT] --replace
gregg list --json
gregg --config PATH <command>
```

Do not add fleet discovery, import from monitoring products, tags/groups, authentication, or remote daemon mutation in version 1.

## Configuration location

Use the platform’s standard per-user config directory through a narrow directory-resolution crate or explicit rules:

```text
Linux: $XDG_CONFIG_HOME/gregg/gregg.toml
       or ~/.config/gregg/gregg.toml
macOS: ~/Library/Application Support/gregg/gregg.toml
```

Support `--config PATH` for tests and nonstandard use. Resolve the path before dispatching subcommands so every operation refers to one explicit file.

Create missing parent directories with user-only permissions where supported. Do not create a config file merely for `gregg list` unless a documented empty-list behavior requires it.

## Config schema

Recommended version-1 shape:

```toml
config_version = 1
refresh_seconds = 5
request_timeout_ms = 1500
max_concurrent_requests = 16
default_port = 11310

[[systems]]
id = "generated-stable-id"
host = "192.168.182.8"
port = 11310
name = "Deadpool"

[[systems]]
id = "generated-stable-id"
host = "macmini.local"
port = 11310
```

Stable IDs prevent selection/state ambiguity if a host or alias is edited later. Use a simple UUID or similarly collision-resistant representation; do not expose database-like complexity.

Validate:

- Supported config version.
- Refresh interval within documented bounds, for example `1..=3600` seconds.
- Request timeout positive and shorter than or sensibly related to refresh behavior.
- Concurrency within a bounded range.
- Default and endpoint ports in `1..=65535`.
- Host is nonempty and contains no URL scheme/path/query.
- Optional display name is nonempty and length bounded.
- Endpoint IDs are unique.
- Exact normalized host/port pairs are unique unless duplicates have a deliberate documented purpose.

The config stores host and port separately. It does not store complete URLs.

## Endpoint parser

Support:

- IPv4 without port: use default port.
- IPv4 with port.
- DNS/mDNS hostname without port.
- DNS/mDNS hostname with port.
- Bracketed IPv6 with port.
- Bare IPv6 without port, if unambiguous under parser rules.

Reject schemes such as `http://`, paths, credentials, whitespace-only input, port zero, overflow, and malformed bracket syntax. Normalize DNS names conservatively without changing case/Unicode semantics in ways that could alter resolution. Normalize IP literals through standard library parsing.

Provide one canonical display representation:

```text
IPv4/DNS: host:port
IPv6: [address]:port
```

Host-only removal semantics:

- `gregg remove 192.168.182.8` removes all entries matching that normalized host, regardless of port.
- `gregg remove 192.168.182.8:11320` removes only the exact endpoint.

If more than one host-only match exists, report the count removed. If no match exists, return a distinct nonzero or documented idempotent success policy; choose one and test it.

## Atomic config store

Implement a `ConfigStore` abstraction with operations such as:

```rust
load_or_default()
load_existing()
write_atomic(&Config)
mutate(|config| -> Result<...>)
```

Writes follow same-directory temporary-file plus rename semantics. Preserve the previous valid config on serialization, permission, flush, or rename failure. Validate the fully mutated config before writing.

Use a lock strategy proportionate to CLI operations. At minimum, detect and avoid lost updates from two concurrent `gregg add` invocations. A short-lived advisory lock file is acceptable if supported/tested across Linux and macOS. Do not leave stale locks that permanently block use.

## Command behavior

### `add`

Parse and normalize the endpoint, assign a stable ID, apply optional name, append in configured order, validate, and atomically persist. Exact duplicates should fail clearly unless `--replace` is implemented.

### `list`

Print one configured system per line in stable order. Recommended text form:

```text
Deadpool  192.168.182.8:11310
          macmini.local:11310
```

Output must remain usable when names are absent and should not perform network requests. `--json`, if included, emits a stable machine-readable representation.

### `remove`

Apply exact or host-wide semantics described above, preserve relative order of remaining entries, and write only when a change occurs.

### `refresh`

Set global polling interval in seconds. This command does not trigger a poll. Reject zero, negative, overflow, and out-of-range input.

### `edit`

Resolve editor in this order:

```text
$VISUAL
$EDITOR
hx
vim
vi
```

Parse environment editor commands carefully. Prefer executing the configured editor as a program with the config path as an argument; if supporting editor arguments in `$VISUAL`/`$EDITOR`, use a shell-word parser and never invoke a shell.

Before launching, ensure a valid config file exists. After the editor exits successfully, reload and validate it. If invalid, report line/column diagnostics and leave the edited file intact for correction; do not silently revert user changes. The TUI should later refuse to start with invalid config rather than overwriting it.

## Error and output conventions

- Normal results to stdout.
- Diagnostics to stderr.
- Distinct error messages for parse, validation, I/O, lock, duplicate, not-found, and editor failures.
- Avoid debug-formatted internal errors in normal output.
- Exit success only when the requested state transition completed or when explicitly documented as idempotent.

Clap help should include concrete examples and default-port behavior.

## Tests

Add table-driven parser tests for IPv4, IPv6, DNS, ports, malformed schemes, and ambiguous cases. Use temporary directories for config-store tests.

Required command-level tests:

- Add first endpoint to missing config.
- Add named and unnamed endpoints.
- Duplicate rejection and optional replace behavior.
- Stable IDs and stable ordering.
- List with empty/missing config.
- Exact endpoint removal.
- Host-wide removal across multiple ports.
- Refresh range validation.
- Atomic-write failure preserves prior file.
- Concurrent mutation does not lose one update.
- Editor resolution order and argument handling.
- Invalid edited TOML reports useful location.
- Non-TUI commands never initialize terminal state.

## Acceptance criteria

Phase 6 is complete when:

1. All required client subcommands are implemented with documented examples and exit behavior.
2. Running `gregg` without a subcommand remains reserved for the future TUI entry point.
3. Configuration uses standard Linux/macOS user paths and supports `--config` overrides.
4. IPv4, IPv6, DNS/mDNS, default-port, and explicit-port parsing are deterministic and tested.
5. Endpoint identity and ordering remain stable across mutations.
6. Add rejects exact duplicates or replaces them only through explicit behavior.
7. Remove distinguishes host-only and exact endpoint forms.
8. Refresh persists a validated global interval.
9. Edit uses a non-shell editor invocation and validates the resulting file.
10. Atomic writes and concurrent mutation controls prevent partial files and ordinary lost updates.
11. List performs no network I/O and remains scriptable.
12. `cargo package -p gregg` includes required config documentation and passes package validation at this stage.

## Handoff to phase 7

Expose validated configuration and endpoint types that can be cloned into the polling engine. Keep file mutation out of polling/state modules; configuration reload should later enter the state engine through an explicit event.
