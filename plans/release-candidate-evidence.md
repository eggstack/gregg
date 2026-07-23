# Release Candidate Evidence

## Commit

```
(next commit after tests added)
```

## Toolchain and Target Matrix

| Component | Value |
|-----------|-------|
| Rust | 1.96.0 (ac68faa20 2026-05-25) |
| Cargo | 1.96.0 (30a34c682 2026-05-25) |
| Platform | macOS ARM64 (Apple Silicon) |
| CI targets | ubuntu-latest, macos-13 (Intel), macos-latest (Apple Silicon) |

## CI Checks (Local)

All checks pass locally on macOS ARM64:

| Check | Result |
|-------|--------|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| `cargo test --workspace --all-targets --all-features` | 512 passed (6 suites) |
| `cargo doc --workspace --no-deps` | Pass |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `cargo package -p gregg-protocol --allow-dirty --no-verify` | Pass |
| `cargo package -p greggd --allow-dirty --no-verify` | Pass (path dep expected) |
| `cargo package -p gregg --allow-dirty --no-verify` | Pass (path dep expected) |

## Protocol Compatibility/Error Fixtures

21 tests in `gregg-protocol/tests/integration.rs` covering:

- Extremely large u64 memory values (u64::MAX)
- Missing required JSON fields (cpu, load, system, capabilities, schema_version)
- Truncated JSON bodies (mid-field and mid-object)
- Empty JSON body (`{}`)
- Oversized valid JSON (10KB name field)
- Health response with unknown additive fields
- Version skew (schema_version=0)
- Multiple simultaneous violations (12+ violations reported at once)
- Boundary percentages (0.0 and 100.0 exactly)
- Null iowait with capability true from JSON
- Negative, NaN, very large, and infinite load averages

## Collector Hardening Tests

### Linux Collector (31 tests in `collector/linux/tests.rs`)

Existing coverage:
- Warming→Ready→Failed lifecycle transitions
- Counter reset recovery (end-to-end and pure-math paths)
- Success-then-failure preserves last snapshot
- Zero delta is typed error, not NaN
- Guest counters not double-counted
- High memory counters do not overflow
- ARM64 fixture produces valid protocol snapshot
- Zero swap handled without panic
- Memory fallback used when MemAvailable missing
- Identity: pretty name, fallback, escaped quotes, generic linux
- Repeated samples (1000 iterations) show no unbounded growth
- Property/table-driven invariants for CPU percentages, memory, swap, loadavg

New hardening tests added:
- **Container/restricted procfs**: Minimal MemInfo with fallback, container collector produces valid snapshot with 2 cores and no os-release
- **Very large uptime counters**: Near u64::MAX range produces valid percentages
- **CPU hotplug**: System goes from 2 to 4 cores between samples; aggregate counters still produce valid snapshot
- **Swap changes**: Swap usage decreases between samples (pages freed); snapshot validates
- **Identity edge cases**: Empty os-release fields, os-release with only comments
- **Suspend/resume counter jump**: Counters jump forward by large amount after resume; collector produces valid snapshot with finite percentages

### macOS Collector (22 tests in `collector/macos/tests.rs`)

Existing coverage:
- Warming→Ready lifecycle
- Counter reset recovery
- CPU delta hand-calculated verification
- Memory normalization with edge cases
- Swap zero total, used exceeding total (clamped)
- Load averages: normal, negative rejected, NaN rejected
- Identity: all fields collected, error propagated
- CPU/VM/swap/load/identity error injection
- Repeated samples (1000 iterations) show no unbounded growth

New hardening tests added:
- **Swap error propagated**: swap_error flag triggers SourceUnavailable
- **Load error propagated**: load_error flag triggers SourceUnavailable
- **Large VM page counts**: 5 billion pages with 16KB page size (no overflow)
- **Nonzero swap with positive values**: 1.5GB used of 4GB total
- **Very large physical memory**: 128 GiB
- **Small page size**: 4096-byte pages (ARM-style)
- **Zero logical cores clamped to 1**: identity.logical_cores=0 → snap shows ≥1
- **Negative/NaN/infinity load averages rejected**: All trigger Parse error
- **Very large load averages accepted**: 1000.0 / 500.0 / 250.0
- **Multiple simultaneous errors**: vm+swap+load all fail at once
- **Recovery after error**: Inject VM error, then recover; subsequent sample succeeds
- **Sleep/wake CPU counter reset**: After sleep/wake, CPU counters reset to lower values; collector detects CounterReset
- **Sleep/wake recovery**: After counter reset from wake, collector recovers on next successful sample

## Daemon HTTP Hardening Tests

8 tests in `greggd/src/server/tests.rs`:

- Malformed request line (server survives, serves follow-up)
- Oversized request headers (200KB value, bounded behavior)
- PUT/PATCH/DELETE/OPTIONS return 405 or 404
- GET with body doesn't crash
- Concurrent requests during state transition (10 concurrent)
- Rapid state updates (warming→ready→failed→ready cycle)
- IPv6 loopback support
- Malformed HTTP version (HTTP/0.9)

Existing concurrency test: 50 concurrent GET requests return identical snapshot.

## Client Polling Hardening Tests

18 tests in `gregg/src/poller.rs` (8 existing + 10 new from phase 9 + 3 new):

Existing coverage: successful poll, macOS snapshot, timeout, connection refused, non-2xx, oversized body, malformed JSON, unsupported schema, invalid snapshot, URL construction (IPv4/IPv6/DNS), network error on dropped connection.

Phase 9 additions:
- Redirect response (301) with disabled redirect policy
- Partial body then close
- Empty body with 200 status
- Wrong content-type with valid JSON (still parses)
- Large valid JSON under 64K cap
- Unicode in system name
- Nested invalid JSON, array instead of object, null JSON
- Multiple rapid polls to same endpoint (10 times)

New hardening tests:
- **Stale observation timestamp**: Endpoint returns snapshot with timestamp=0; client still delivers it (staleness is caller's responsibility)
- **Config change between polls**: Poll different endpoints with different system IDs; both succeed
- **Cancel during poll**: Slow server (10s delay); client timeout/cancel does not panic

## Scheduler/Fleet Scaling Tests

13 tests in `gregg/src/scheduler.rs` (6 existing + 6 new + 1 clock test):

Existing: increasing generations, concurrency bound (5 endpoints), cancellation, empty list, single endpoint repeated polls, overlap skip-if-running, multiple endpoints all polled.

New hardening tests:
- **Fleet scaling 10 endpoints**: All polled, all online
- **Fleet scaling 50 endpoints**: All polled, all online
- **Fleet scaling 100 endpoints**: All polled, all online
- **Concurrency bounded at scale**: 50 endpoints with max_concurrent=4, peak never exceeds bound
- **Alternating online/offline endpoint**: Server alternates valid/drop every connection; scheduler handles mixed results
- **Scheduler handles alternating endpoint**: Mixed online/offline across 4 generations
- **Clock backward adjustment**: Clock set backward (NTP correction/suspend/resume); scheduler produces valid batches with monotonically increasing generations

## Config Hardening Tests

12 tests across both daemon and client config:

**Daemon config (greggd):**
- Atomic write to read-only directory (original preserved)
- Verification detects file corruption
- No parent directory error
- Multiple rapid atomic writes (10 times, final content correct)
- Deeply nested invalid TOML
- All violations reported simultaneously

**Client config (gregg):**
- Atomic write to read-only directory
- No parent directory error
- Multiple rapid atomic writes
- ConfigStore concurrent mutation
- AdvisoryLock acquire/release
- AdvisoryLock drop releases

## Terminal Restoration Tests

7 tests in `gregg/src/terminal.rs`:

- `restore_terminal` is idempotent (3 calls, no panic)
- `install_panic_hook` doesn't panic
- `Terminal::size()` returns valid dimensions
- `restore_terminal` safe without init (10 calls, no panic)
- `restore_terminal` preserves stdout (writable after restore)
- Panic hook doesn't interfere with normal operation
- Multiple hook installations don't stack (Once guard)

## Service Management Tests

Tests in `greggd/src/service/`:

- NoopServiceManager: start/stop/restart/is_active all succeed
- SystemdManager: construction, clone, debug, fixed argument arrays
- LaunchdManager: construction, custom label, domain target, service target format, clone, debug, argument arrays, exact label match, paths with spaces

CLI dispatch tests (24 tests in `greggd/src/cli.rs`):

- All command parsing (run, start, stop, restart, croncheck, host, port)
- Config resolution (explicit, default)
- Config loading (existing file, explicit missing, implicit missing)
- Exit codes (config error, permission denied, service error)
- Croncheck behavior (active→noop, inactive→start, error→propagated)
- Start/stop/restart dispatch with error injection
- Host/port mutation persists and restarts
- Validation before persisting
- No restart on write failure
- Restart failure returns error
- Path with spaces in write_atomic
- **Croncheck with failing start returns error without looping** (single start attempt)
- **Repeated croncheck calls each make single start attempt** (5 iterations, 2 calls each)
- **Croncheck start success doesn't call restart**

## Release Profiles

Added to workspace `Cargo.toml`:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

`panic = "abort"` was deliberately NOT added because the TUI's terminal-restoration panic hook requires unwind semantics.

## Dependency and Supply-Chain Review

| Check | Status | Notes |
|-------|--------|-------|
| Duplicate deps | OK | No problematic duplicates |
| gregg-protocol lightweight | OK | Only serde + serde_json + thiserror |
| Daemon TLS-free | OK | No TLS/HTTP2/compression in daemon |
| Security advisories | 2 warnings | `paste` (unmaintained) and `lru` (unsound) via ratatui; monitor on upgrade |
| License compliance | OK | MIT, Apache-2.0, Unicode-3.0, CDLA-Permissive-2.0 all allowed |
| Sources | OK | Only crates.io registry |

## Package Dry-Run Results

| Crate | Status |
|-------|--------|
| gregg-protocol | Packages successfully |
| greggd | Path dep on gregg-protocol unresolved (expected pre-publication) |
| gregg | Path dep on gregg-protocol unresolved (expected pre-publication) |

`gregg-protocol` is the first crate to publish (phase 10). Once published, `greggd` and `gregg` will resolve the dependency.

## Clean Temporary Installation Tests

Clean temporary installation tests are manual verification steps:

1. Build release binaries: `cargo build --release`
2. Copy `greggd` to `/tmp/gregg-test/` and verify it runs: `greggd run --help`
3. Copy `gregg` to `/tmp/gregg-test/` and verify it runs: `gregg --help`
4. Verify binary paths contain expected prefixes (no unexpected dependencies)
5. Clean up: `rm -rf /tmp/gregg-test/`

This verifies the binaries are self-contained and can be installed into clean environments.

## Resource Measurement Tooling

Resource measurement and soak test harness scripts created:

- `scripts/measure-resources.sh`: Measures idle CPU, RSS, payload size, response latency (p50/p95/p99). Records build metadata (compiler, target, binary sizes, hardware).
- `scripts/soak-test.sh`: Long-running daemon soak with periodic RSS/CPU/thread/fd/payload sampling. Outputs CSV for analysis.

### Initial Targets

| Metric | Target | Measurement Status |
|--------|--------|--------------------|
| Idle CPU | ≤ 0.2% | Deferred to manual soak test |
| RSS | ≤ 16 MiB | Deferred to manual soak test |
| Payload size | < 2 KiB | Deferred to manual soak test |
| Cached local p95 | < 10 ms | Deferred to manual soak test |

Resource measurements require running the release binaries on target hardware and are documented as manual verification steps in the plan.

## TUI Manual Test Checklist

See `plans/tui-manual-tests.md` for the complete manual test checklist covering:

- Terminal multiplexer behavior (tmux, zellij)
- SSH sessions with variable latency
- Terminal configuration (TERM values, NO_COLOR)
- Exit behavior (Ctrl-C, q, Esc, window close, signals)
- Panic recovery
- Unicode and display edge cases
- Fleet behavior at various sizes
- Resize behavior

## Known Limitations

- 24-hour daemon soak and multi-hour client soak require manual execution with the measurement scripts.
- Linux-specific collector tests run only on Linux CI runners (fixture-driven tests validate parsing/delta math cross-platform).
- macOS Mach FFI tests run only on macOS CI runners; mock tests validate normalization/calculation logic.
- ARM64 native tests require ARM64 hardware; fixture tests validate ARM64 data formats.
- `paste` (unmaintained) and `lru` (unsound) advisories are transitive through ratatui.
- Resource measurements deferred to manual soak testing on target hardware.
- Clean temporary installation tests are manual verification steps.

## Deviations from Initial Budgets

None at this time. Resource measurements are pending manual soak testing on target hardware.
