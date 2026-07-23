# Gregg version-1 roadmap

## 1. Purpose

This roadmap takes `gregg` from an empty repository to a stable version-1 release containing three crates published on crates.io:

- `gregg-protocol`, a lightweight library defining the JSON contract shared by daemon and client.
- `greggd`, a Linux/macOS native metrics daemon with systemd and launchd lifecycle integration.
- `gregg`, a compact Ratatui client with endpoint-management commands, bounded concurrent polling, and four-line-per-host rendering.

The roadmap intentionally optimizes for a narrow operational product: low overhead, local/private-network deployment, predictable behavior, and maintainable crate boundaries. It does not target feature parity with htop, btop, Glances, Netdata, Prometheus, or remote-management suites.

## 2. Version-1 product definition

Version 1 is successful when a user can install `greggd` on supported Linux and macOS machines, configure a bind address and valid TCP port, run it under the native service manager, add those endpoints to `gregg`, and observe all reachable systems in a small terminal pane.

Each reachable system must expose and render:

- Stable user-facing system name and hostname.
- Operating-system identity, kernel identity, and architecture.
- Logical CPU core count.
- Total CPU utilization derived from interval deltas.
- One-, five-, and fifteen-minute load averages.
- Used and total memory.
- Used and total swap.
- Linux aggregate CPU I/O-wait percentage where supported.
- Explicit capability metadata where a metric is unsupported, including macOS I/O wait.

A reachable system consumes exactly four terminal rows. An unreachable system consumes one row and is displayed after all reachable systems. The client defaults to a five-second refresh interval and supports keyboard navigation with `j`/Down and `k`/Up.

## 3. Architectural invariants

### 3.1 Workspace and dependency direction

The repository is a Cargo workspace with three publishable members under `crates/`:

```text
crates/gregg-protocol
crates/greggd
crates/gregg
```

Allowed dependency direction:

```text
gregg-protocol <- greggd
gregg-protocol <- gregg
```

Disallowed:

- `gregg-protocol` depending on either binary crate.
- `greggd` depending on `gregg`.
- `gregg` depending on `greggd`.
- Shared implementation code being placed in the protocol crate merely to avoid creating an internal module.

### 3.2 Daemon data path

```text
native collector -> periodic sampler -> immutable cached snapshot -> HTTP serializer
```

HTTP requests never initiate native collection. CPU utilization and Linux I/O wait require at least two native counter samples, so readiness remains false until a valid delta-derived snapshot exists.

### 3.3 Client data path

```text
configuration -> poll scheduler -> completed batch -> state reducer -> renderer
```

The renderer performs no network or filesystem I/O. Poll tasks do not directly mutate widgets or terminal state. A completed polling generation is applied atomically enough that old results cannot overwrite newer configuration or newer batches.

### 3.4 Platform behavior

Linux and macOS are first-class daemon targets for version 1.

Linux collection uses native procfs/kernel identity interfaces. macOS collection uses Mach host statistics and sysctl APIs through a contained FFI boundary. External commands are not daemon runtime dependencies.

Service lifecycle is delegated to systemd on Linux and launchd on macOS. `greggd run` remains a foreground process. No self-daemonization or PID files are introduced.

### 3.5 Protocol behavior

The API is versioned from the first implementation. Numeric values are transported as raw units, primarily bytes and percentages. Formatting belongs to the client.

Unsupported metrics are represented as absent/null and paired with capability metadata. A null value is not interchangeable with zero. The TUI should not require platform-specific branches to interpret this distinction.

## 4. Scope boundaries

Version 1 includes only the metrics and management surfaces required by the project contract. The following are deferred:

- Per-process metrics or process control.
- Disk/network throughput or filesystem inventory.
- Historical persistence, graphs, or aggregation.
- Alerting and notifications.
- Remote command execution or daemon reconfiguration over HTTP.
- Discovery protocols or automatic fleet enrollment.
- Authentication, multi-user access control, and public-internet exposure.
- TLS issuance or certificate management.
- Web UI, plugins, Prometheus compatibility, and third-party exporters.

Security work remains proportionate to private-LAN/SSH/overlay use: bounded requests, read-only endpoints, safe parsing, explicit bind configuration, no command endpoint, and sound service permissions.

## 5. Phase sequence

### Phase 1: foundation, workspace, and protocol

Create the workspace, crate skeletons, package metadata, error conventions, quality gates, and schema version 1. Define normalized identity, capabilities, CPU, load, memory, swap, timestamps, and readiness/error structures. Establish fixtures and synthetic snapshot builders used by later phases.

Exit condition: `gregg-protocol` is independently packageable and its serialized contract is protected by compatibility tests.

### Phase 2: Linux collector

Implement Linux identity and interval-based metrics using native kernel interfaces. Define exact CPU busy, I/O-wait, memory availability, swap, load, counter reset, and warming-up semantics. Build fixture-driven tests that do not depend on the CI host’s instantaneous load.

Exit condition: Linux native samples normalize into valid protocol snapshots with deterministic tests.

### Phase 3: macOS collector

Implement safe wrappers for Mach and sysctl interfaces, then normalize CPU, load, memory, swap, and identity into the same protocol. Keep all required unsafe code in a minimal FFI module. Mark I/O wait unsupported.

Exit condition: Intel and Apple Silicon CI exercise native collection while pure normalization tests cover edge cases.

### Phase 4: daemon sampler and HTTP API

Build the platform-selected collector, periodic sampling loop, immutable snapshot cache, readiness state, read-only routes, explicit timeouts/limits, structured logging, and graceful shutdown. Test the server through synthetic collectors.

Exit condition: `greggd run` exposes stable cached JSON and readiness without collection-on-request.

### Phase 5: daemon configuration, lifecycle, and packaging

Implement platform-appropriate config paths, atomic mutation, `start`, `stop`, `restart`, `croncheck`, `host`, and `port`. Add systemd and launchd templates/installers, privilege expectations, service-user policy, and packaging validation.

Exit condition: fresh Linux and macOS installations can persist configuration and survive service restart/reboot behavior.

### Phase 6: client configuration and non-TUI CLI

Implement user config discovery and mutation, endpoint parsing/normalization, aliases, stable ordering, `add`, `list`, `remove`, `refresh`, and `edit`. Ensure every non-TUI command is scriptable and never enters raw terminal mode.

Exit condition: endpoint configuration is reliable, atomic, validated, and covered by command-level tests.

### Phase 7: polling and state engine

Build one reusable HTTP client, fixed-concurrency batch polling, explicit request timeout, generation handling, reachability classification, stable online/offline ordering, selection, and variable-height viewport state. Keep clock and scheduler behavior testable.

Exit condition: the complete non-visual client core handles mixed success/failure without stale-result races.

### Phase 8: compact Ratatui interface

Implement terminal lifecycle, event handling, resize adaptation, four-line online blocks, one-line offline blocks, priority-aware width reduction, bars, semantic scrolling, keyboard controls, and immediate refresh. Add buffer/golden tests at multiple dimensions.

Exit condition: mixed fleets remain usable in small multiplexer panes and the terminal is always restored.

### Phase 9: hardening, performance, and release-candidate evidence

Exercise malformed responses, unsupported schemas, slow and unreachable hosts, counter anomalies, service restarts, suspend/resume, config corruption, terminal resizing, and large endpoint sets. Measure CPU, RSS, binary/package size, response latency, and redraw behavior on representative Linux SBC/server and macOS targets.

Exit condition: documented measurements meet or justify the version-1 budgets and all release targets pass CI and package-install tests.

### Phase 10: crates.io release closure

Finalize public documentation, license, changelog, package manifests, semver policy, API examples, install instructions, release automation, provenance expectations, and publication order. Dry-run and inspect every package before publishing protocol, daemon, then client.

Exit condition: crates.io contains compatible `1.0.0` releases of all three crates, source tags match published contents, and clean-machine installation is verified.

## 6. Cross-phase quality gates

Each phase must maintain:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

As crates become packageable, also require:

```text
cargo package -p gregg-protocol --allow-dirty
cargo package -p greggd --allow-dirty
cargo package -p gregg --allow-dirty
```

`--allow-dirty` is acceptable only inside CI verification before publication; release commands must use a clean tree.

CI should eventually include:

- Linux x86-64 stable Rust.
- macOS x86-64 stable Rust.
- macOS ARM64 stable Rust.
- Linux ARM64 compile/package verification, with native execution where infrastructure is available.
- Minimum supported Rust version verification after the MSRV is declared.

## 7. Initial resource budgets

These are engineering targets rather than promises. Phase 9 must measure them and document exceptions.

- `greggd` idle CPU: approximately 0.2% or less on a typical host at a one-second sample cadence.
- `greggd` resident memory: approximately 16 MiB or less where the platform/runtime permits.
- Status JSON payload: below 2 KiB for schema version 1.
- Cached local status response: p95 below 10 ms under ordinary local load.
- `gregg` idle CPU between refreshes: approximately 1% or less.
- Poll concurrency: fixed and bounded independently of configured host count.
- Redraw: event-driven or capped; no continuous high-FPS loop.

Binary size should be tracked per target, not optimized through fragile code until measurement shows a deployment problem.

## 8. Principal risks and controls

### Semantic drift across operating systems

Memory and CPU accounting differ across Linux and macOS. Control this by documenting normalized semantics, carrying capabilities, retaining raw platform calculations in isolated collectors, and using cross-platform contract tests.

### macOS FFI correctness

Mach/sysctl APIs require unsafe calls and structure-length handling. Control this by constraining unsafe code to one module, checking all return statuses and lengths, returning owned values, and exercising native CI on both macOS architectures.

### Service-management privilege ambiguity

System services and system configuration usually require elevation. Control this with explicit installer behavior, actionable permission errors, `--config` development overrides, and no hidden privilege escalation.

### Poll-cycle amplification

A large fleet could create synchronized requests or long batches. Control this with bounded concurrency, per-request timeout, one reusable client, generation tracking, and eventual optional jitter only if measurements justify it.

### TUI layout regressions

Variable-height entries and narrow panes are prone to clipping and selection errors. Control this with logical-entry scrolling, pure rendering functions, buffer tests over a dimension matrix, and terminal-area checks on every frame.

### Publication coupling

Three crates can fail packaging because path dependencies or metadata differ. Control this from phase 1 by combining `path` and `version` dependencies, running `cargo package` continuously, and publishing in dependency order.

## 9. Versioning and compatibility policy

All crates begin pre-release development below 1.0.0. The shared protocol schema version is independent from crate package versions but must be documented and tested.

Before 1.0.0, breaking Rust APIs may occur between planned phases, but wire-schema changes must still be intentional because daemon and client may be deployed independently. At 1.0.0:

- The version-1 JSON schema is stable.
- The documented CLI commands and config behavior are stable.
- Additive optional protocol fields are permitted under the compatibility policy.
- Breaking schema changes require a new schema major and compatibility/migration handling.
- Crate semver follows normal public Rust API expectations.

## 10. Definition of done

Gregg version 1 is complete only when all of the following are true:

1. All three crates are independently packageable and published as compatible version `1.0.0` releases.
2. Linux and macOS daemons pass native collection, service, and API tests on supported architectures.
3. The client can configure and concurrently poll mixed Linux/macOS fleets.
4. The TUI satisfies four-row online, one-row offline, width adaptation, ordering, and keyboard navigation contracts.
5. Unsupported macOS I/O wait is represented explicitly rather than as zero.
6. Fresh-install documentation is validated on at least one Linux x86-64 host, one Linux ARM64 host, one Intel Mac or CI equivalent, and one Apple Silicon Mac.
7. Resource measurements and known limitations are recorded.
8. Source tags, crates.io package contents, and release notes correspond exactly.
9. No deferred non-goal has entered the release without an explicit scope decision.
