# gregg-update

Internal workspace infrastructure: the shared binary-first self-update mechanism used by `gregg update` and `greggd update` (Plan 104).

Not a user-facing product. It owns cross-program update mechanics only — stable-version parsing/comparison, supported-target mapping, release asset naming/URL construction, `curl`/Cargo discovery and bounded execution, SHA-256 verification, staged candidate validation, staging lifetime, executable replacement, and the shared update error/outcome primitives.

Daemon activation/restart policy stays in `greggd`; CLI outcome presentation stays in each application crate. The protocol crate is deliberately not involved.
