# Release Candidate Evidence

## Commit

```
7709613ec5db32bbc5e6b3094e4cc4ed0748caaf
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
| `cargo test --workspace --all-targets --all-features` | 480 passed (6 suites, 1.07s) |
| `cargo doc --workspace --no-deps` | Pass |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `cargo package -p gregg-protocol --allow-dirty --no-verify` | Pass (15 files) |
| `cargo package -p greggd --allow-dirty --no-verify` | Pass (32 files) |
| `cargo package -p gregg --allow-dirty --no-verify` | Pass (25 files) |

## Protocol Compatibility/Error Fixtures

Added 21 new tests to `gregg-protocol/tests/integration.rs` covering:

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

Existing collector tests cover:
- Warming→Ready→Failed lifecycle transitions
- Counter reset recovery
- Success-then-failure preserves last snapshot
- Invalid metrics coalescing (NaN→0.0)
- Consecutive failure tracking

Sampler tests use `SyntheticClock` and `SyntheticCollector` for deterministic behavior without real wall-clock sleeps.

## Daemon HTTP Hardening Tests

Added 8 new tests to `greggd/src/server/tests.rs`:

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

Added 10 new tests to `gregg/src/poller.rs`:

- Redirect response (301) with disabled redirect policy
- Partial body then close
- Empty body with 200 status
- Wrong content-type with valid JSON (still parses)
- Large valid JSON under 64K cap
- Unicode in system name
- Nested invalid JSON, array instead of object, null JSON
- Multiple rapid polls to same endpoint (10 times)

## Config Hardening Tests

Added 12 new tests across both daemon and client config:

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

Added 3 new tests to `gregg/src/terminal.rs`:

- `restore_terminal` is idempotent (3 calls, no panic)
- `install_panic_hook` doesn't panic
- `Terminal::size()` returns valid dimensions

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
| Duplicate deps | OK | No problematic duplicates (syn v2/v3, hashbrown v0.15/v0.17 are standard ecosystem transitions) |
| gregg-protocol lightweight | OK | Only serde + serde_json + thiserror |
| Daemon TLS-free | OK | No TLS/HTTP2/compression in daemon |
| Security advisories | 2 warnings | `paste` (unmaintained) and `lru` (unsound) via ratatui; monitor on upgrade |
| License compliance | OK | MIT, Apache-2.0, Unicode-3.0, CDLA-Permissive-2.0 all allowed |
| Sources | OK | Only crates.io registry |

## Package Dry-Run Results

| Crate | Files | LICENSE included | Status |
|-------|-------|-----------------|--------|
| gregg-protocol | 16 | Yes | Packages successfully |
| greggd | 32 | Yes | Path dep on gregg-protocol unresolved (expected pre-publication) |
| gregg | 25 | Yes | Path dep on gregg-protocol unresolved (expected pre-publication) |

`gregg-protocol` is the first crate to publish (phase 10). Once published, `greggd` and `gregg` will resolve the dependency. No generated files, fixtures, secrets, or local paths leak into packages.

## Known Limitations

- 24-hour daemon soak and multi-hour client soak tests require manual execution and are documented in the plan but not automated.
- Linux-specific collector tests (procfs edge cases) run only on Linux CI runners.
- macOS Mach FFI tests run only on macOS CI runners.
- ARM64 native tests require ARM64 hardware (CI runner or SBC).
- `paste` (unmaintained) and `lru` (unsound) advisories are transitive through ratatui and will be resolved on ratatui upgrade.

## Deviations from Initial Budgets

None at this time. Resource measurements (idle CPU, RSS, payload size) are deferred to manual soak testing documented in the plan.
