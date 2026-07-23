# Phase 9: testing, hardening, and performance verification

## Objective

Convert the feature-complete workspace into a defensible release candidate through cross-platform failure testing, service/install verification, resource measurement, long-running behavior checks, and package-content validation.

This phase is evidence-driven. It should not add broad new features. Any proposed change must address correctness, resilience, operability, packageability, or a measured violation of the lightweight product goal.

## Test matrix

Maintain a documented matrix covering:

```text
Linux x86-64        native unit/integration/service/package tests
Linux ARM64         native SBC or ARM runner tests where available
macOS x86-64        native collector/launchd/package tests
macOS ARM64         native collector/launchd/package tests
```

Cross compilation is useful but does not replace native execution for Mach FFI, service managers, filesystem paths, terminal behavior, or runtime resource measurements.

Use at least one representative resource-constrained Linux device, ideally a Raspberry Pi-class ARM64 host, for daemon and client measurements.

## Protocol and compatibility hardening

Test:

- Canonical version-1 Linux and macOS payloads.
- Unknown additive JSON fields.
- Missing required fields.
- Unsupported schema version.
- Unknown capability combinations.
- Null I/O wait with capability true and numeric I/O wait with capability false.
- NaN/infinity attempts where the JSON parser permits related invalid forms.
- Extremely large integers and percentages.
- Body truncation and oversized bodies.
- Daemon/client version skew within planned supported combinations.

Build a compatibility fixture suite that can be retained for future releases. Once version 1 is published, these fixtures become regression contracts.

## Collector hardening

### Linux

Exercise:

- Missing, unreadable, and malformed procfs sources.
- CPU counter reset/decrease and zero delta.
- Very large uptime/counter values.
- Hotplug or logical-core count changes if observable.
- Missing `MemAvailable` fallback.
- Zero swap, swap changes, and total/free anomalies.
- Containerized/restricted procfs behavior.
- Suspend/resume where counters or scheduler intervals jump.

### macOS

Exercise:

- Mach call failure injection and unexpected returned counts.
- Sysctl missing key, short buffer, size change between query/read, invalid string, and permission error.
- Intel and Apple Silicon structure/layout behavior.
- CPU tick reset/zero delta.
- Large page counts, page-size arithmetic, and memory-total anomalies.
- Zero and nonzero swap.
- Suspend/resume and sleep/wake transitions.
- Product-version fallback behavior.

Run sanitizers or Miri on pure safe modules where practical. Miri may not execute Darwin FFI, but the safe normalization layer should still be testable. Consider AddressSanitizer on native macOS integration tests if toolchain/support effort is proportionate.

## Daemon and HTTP hardening

Test:

- Bind conflict and invalid address.
- Rapid start/stop/restart.
- Graceful shutdown with in-flight requests.
- Collector warming, failure before first snapshot, failure after success, and recovery.
- Snapshot staleness and health transitions.
- Many simultaneous cached GET requests.
- Slowloris-like incomplete headers within configured limits.
- Unsupported methods and request bodies.
- Malformed request lines and abrupt disconnects.
- Repeated connection churn.
- Listener behavior on IPv4 and IPv6 loopback if supported.
- Service restart loops caused by invalid config; ensure native service policy does not spin aggressively.

Use a local load generator or purpose-built integration test. Do not expand the daemon into a hardened public web server; verify that its narrow local read-only surface fails safely.

## Client polling hardening

Create deterministic mock endpoints for:

- Fast valid Linux and macOS responses.
- Timeout, refused connection, reset, DNS failure, and partial body.
- Redirect loops.
- Non-2xx statuses.
- Invalid content type with otherwise valid JSON; decide documented behavior.
- Malformed/oversized JSON.
- Valid JSON with invalid protocol invariants.
- Endpoint alternating online/offline every cycle.
- Endpoint returning stale observation timestamps.
- More endpoints than the concurrency limit.
- Configuration change during active batch.
- Quit during active requests.

Test endpoint sets of at least 1, 10, 50, 100, and a higher stress count appropriate to the test host. Verify bounded memory/tasks and no stale-generation overwrites.

## TUI hardening

Beyond phase-8 buffer tests, run interactive/manual checks in:

- tmux and zellij small panes.
- Rapid pane resizing.
- SSH sessions with variable latency.
- Common `TERM` values and monochrome/no-color settings.
- Terminal close or SIGHUP.
- Ctrl-C and panic injection.
- Unicode and malformed-looking but valid names.
- Fleet reorder while selected host changes reachability.
- Empty config and all-hosts-offline state.

Verify no alternate-screen/raw-mode residue after all exit paths. Add a small automated terminal lifecycle harness where feasible.

## Configuration and service hardening

Test atomic writes under:

- Destination permission denial.
- Temporary-file creation failure.
- Serialization failure injection.
- Flush failure where injectable.
- Rename failure.
- Concurrent writers.
- Process termination before rename.
- Invalid manually edited TOML.

Installation/service checks:

- Fresh install.
- Reinstall/upgrade preserving config.
- Start/stop/restart/croncheck.
- Reboot persistence.
- Uninstall behavior and retained config policy.
- Binary path with expected prefixes.
- macOS Application Support path containing spaces.
- systemd and launchd logs useful for invalid config and bind failure.

Record commands and evidence in a release-candidate verification document or CI artifacts.

## Resource measurements

Measure release-profile, stripped binaries on each target where practical. Record compiler version, target, optimization settings, hardware, and sampling/poll configuration.

### `greggd`

Measure:

- Idle CPU at one-second sampling for at least 30 minutes.
- RSS after startup and after a long soak.
- Native sample duration distribution.
- Cached endpoint p50/p95/p99 latency.
- Response payload size.
- Behavior with 1, 10, and many concurrent clients.
- File descriptors and thread/task count stability.

Initial targets:

```text
idle CPU             approximately <= 0.2% on typical hosts
RSS                  approximately <= 16 MiB where platform permits
status payload       < 2 KiB
cached local p95     < 10 ms
```

Treat target misses as investigation triggers. Platform runtime/accounting differences may justify revised documented budgets, especially RSS on macOS.

### `gregg`

Measure:

- Idle CPU between five-second polls.
- CPU during polling batches of 10/50/100 hosts.
- RSS at those fleet sizes.
- Request/task/connection bounds.
- Render count per minute while idle.
- Resize and key-repeat behavior.
- Startup-to-first-render and startup-to-first-complete-batch.

Initial target: approximately <= 1% idle CPU and no continuous redraw loop.

## Soak and fault tests

Run:

- `greggd` continuously for at least 24 hours on Linux and macOS.
- `gregg` for multiple hours polling a mixed mock/native fleet.
- Repeated endpoint restarts and network disconnects.
- System sleep/wake on a Mac and, where relevant, Linux suspend/resume.
- Clock adjustment forward/backward to confirm monotonic scheduling is not corrupted.
- Configuration edits and endpoint additions/removals during runtime if reload is supported.

Track RSS, task/thread count, file descriptors, log volume, and refresh continuity. Any monotonic resource growth requires explanation or correction.

## Dependency and supply-chain review

Run and review:

- `cargo tree` for unexpected heavy/default features.
- Duplicate dependency versions.
- `cargo deny` or equivalent checks for advisories, licenses, bans, and sources.
- `cargo audit` where useful.
- Package contents for generated files, fixtures, secrets, or local paths.

Confirm HTTP dependencies do not pull a TLS stack when version-1 plain HTTP is intended. Confirm the protocol crate remains dependency-light.

Do not blindly reject all duplicate transitive versions; document meaningful size/security issues and fix them where compatible.

## Build profiles

Define intentional release profiles. Consider:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
panic = "abort"
```

Evaluate `panic = "abort"` carefully: the TUI’s terminal-restoration panic hook cannot run after an abort, so the client may need unwind semantics even if the daemon uses abort. Cargo profiles apply broadly unless package overrides are used. Choose behavior based on terminal safety and measured size, not assumption.

Measure before adopting aggressive size options that materially increase build time or complicate debugging.

## Release-candidate evidence document

Create a final evidence file containing:

- Commit SHA.
- Toolchain and target matrix.
- CI links/results.
- Native service install results.
- Resource measurement table.
- Soak-test duration/results.
- Known limitations.
- Deviations from initial budgets and rationale.
- Package dry-run results.

Do not mark acceptance criteria complete without reproducible evidence references.

## Acceptance criteria

Phase 9 is complete when:

1. Linux x86-64, Linux ARM64, macOS x86-64, and macOS ARM64 have the documented level of native evidence.
2. Protocol compatibility/error fixtures cover malformed, additive, skewed, and unsupported payloads.
3. Native collectors recover from transient/reset conditions and fail safely on malformed sources/API failures.
4. Daemon HTTP limits and shutdown behavior withstand concurrency and malformed-request tests without panic or unbounded growth.
5. Client polling remains bounded and generation-correct through large fleets and active configuration changes.
6. Terminal restoration is verified across normal, signal, error, and panic paths.
7. Atomic config and service installation survive failure injection and upgrade/reinstall tests.
8. Resource measurements are recorded and either meet targets or include accepted rationale/corrective action.
9. At least one daemon 24-hour soak and one multi-hour client soak show no unexplained monotonic resource growth.
10. Dependency/license/advisory review has no unresolved release-blocking findings.
11. All three crates pass package dry runs and clean temporary installation tests.
12. A release-candidate evidence document ties every result to an exact commit.

## Handoff to phase 10

Freeze features after this gate. Only release-blocking correctness, documentation, packaging, or security fixes should land before publication, and any such fix must rerun affected evidence.
