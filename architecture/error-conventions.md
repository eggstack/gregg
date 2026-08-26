# Error conventions

Each binary crate (`greggd`, `gregg`) establishes a crate-local typed error
boundary using `thiserror`. Internal errors stay internal: application code
returns the typed error, command entry points render concise diagnostics, and
`std::error::Error` chains remain available for tracing/debug logs.

Wire-protocol errors are a separate concern. The protocol crate does not
expose application errors. Public wire responses carry structured, safe
information only:

- A machine-readable category (e.g. `warming`, `collector_failure`).
- A short human-readable message that does not embed filesystem paths,
  platform-private structures, or internal error chains.

Command entry points follow these rules:

- Human-readable output goes to `stdout`.
- Diagnostics, warnings, and recoverable errors go to `stderr`.
- Exit codes are meaningful and scriptable: success, configuration error,
  runtime error, etc. Exact codes are defined per command in their phase plan.
- Configuration writes are atomic and validated/reloaded after persistence.

`greggd` keeps process termination at `src/main.rs`. Its reusable CLI and
runtime paths return errors without printing or exiting; the binary formats a
failure once and classifies it as configuration (`1`), service (`2`), runtime
(`3`), or permission denied (`4`). Logging is also initialized at that binary
boundary with a fallible subscriber setup, so embedding and tests remain safe
when a global subscriber already exists.

Windows SCM dispatcher failures return to `main` for the ordinary diagnostic
and exit-code boundary. Once the generated `ServiceMain` callback has been
invoked, worker failures are logged once by that callback, represented by a
best-effort `STOPPED` status with a nonzero exit code, and are not returned
through ordinary `main` propagation. Stop and Shutdown are forwarded to the
shared daemon runtime through a nonblocking one-shot signal.

The Gregg TUI treats a closed Systems scheduler command channel as a runtime
error. A successful config reload sends its endpoint replacement through the
bounded channel before committing the corresponding `AppState` reconciliation;
channel pressure therefore waits for capacity instead of silently allowing the
displayed endpoint set and scheduler endpoint set to diverge.

The protocol crate's own validation surface is structured: a `validate()`
method returns a list of violations rather than panicking or wrapping serde
deserialization with opaque checks. This keeps forward compatibility
manageable when additive fields appear in future schema versions.

## Collector errors

The collector module (`crates/greggd/src/collector/error.rs`) defines
`CollectErrorKind` with these variants:

- **Warming** — first sample not yet available; counters have no delta.
- **SourceUnavailable** — a procfs/sysfs entry is missing or unreadable.
- **Parse** — a metric file was present but its content could not be parsed.
- **CounterReset** — a kernel counter wrapped or decreased since the last
  sample, invalidating the delta.
- **Numeric** — an arithmetic error (e.g. division by zero) during
  normalisation.
- **IdentityFallback** — a system-identity field could not be read. If this
  reaches the sampler, the cycle fails rather than publishing a blank or
  fabricated identity.

These are crate-local typed errors that never appear on the wire. Wire
responses carry the coarse `HealthCategory` (`Warming`,
`CollectorFailure`, `NotServing`) defined in `gregg-protocol`.
