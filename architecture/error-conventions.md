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
- **IdentityFallback** — a system-identity field could not be read and a
  fallback value was used.

These are crate-local typed errors that never appear on the wire. Wire
responses carry the coarse `HealthCategory` (`Warming`,
`CollectorFailure`, `NotServing`) defined in `gregg-protocol`.
