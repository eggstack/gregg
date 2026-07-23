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
