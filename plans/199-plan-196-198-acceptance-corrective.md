# Plan 199: Plans 196–198 acceptance corrective pass

Status: in progress; eight of nine acceptance criteria pass. The effective local pipe ACL, the hosted Windows CI gates, and the footprint criterion are evidenced; the remaining item is the actual remote-client rejection, which requires a two-host Windows run. Plans 162, 196, 197, and 198 retain distinct acceptance statuses.

Source: follow-up to the recorded Plan 198 test, Clippy, local-check, Windows, and footprint failures, plus the independent Plan 196 post-rename atomic-writer error edge and Plan 197 qualification blockers.

Depends on: the implementations and evidence recorded by Plans 196, 197, and 198, and Plan 162's footprint contract as prospectively amended to 3,700,000 bytes. This is corrective qualification, not a new product feature.

## Objective

Resolve and qualify the outstanding acceptance issues for the scheduled-job CLI/config editor (Plan 196), local rehash (Plan 197), and auto-rehash/configtest (Plan 198). Keep each plan’s acceptance line distinct: evidence may be shared where it proves the same gate, but a pass for one plan must not be represented as closing another plan’s unique criteria.

The current recorded Plan 198 full `greggd` run is 567 passed and 10 failed: eight Unix control-socket path/identity tests and two uninstall purge tests that attempted to remove `/var/log/greggd.log`. Strict Clippy reports 15 `borrow_as_ptr` findings and one `too_many_lines` finding in `gregg-host/src/macos/ffi.rs`. The attempted `./scripts/check-local.sh` run was interrupted and has no completed result. Native Windows named-pipe ACL/remote-client and SCM qualification is outstanding. The earlier Darwin arm64 release measurement was 3,555,712 bytes, above the then-current 3,400,000-byte ceiling; a subsequent explicit user-approved amendment sets the prospective ceiling to 3,700,000 bytes, under which the measured release is within limit.

Plan 196 also has a separately recorded writer contract defect: a post-rename permission or parent-directory sync error can be returned after the live config has already been replaced. Plan 199 must resolve this edge without implying that a returned error always means the old file remains intact.

## Scope and guardrails

In scope:

- Correct Unix control-socket test fixtures so they deliberately exercise primary and fallback endpoints without depending on the host’s `TMPDIR` length.
- Isolate uninstall purge tests from machine paths, and ensure purge preflight covers every path that execution may remove before service/config teardown begins.
- Resolve Plan 196’s post-rename writer outcome ambiguity and add deterministic regression coverage.
- Clear the current macOS FFI Clippy findings without broad lint suppression or behavioral changes.
- Complete the interrupted local verification and the applicable full test/strict-Clippy gates.
- Obtain native Windows evidence for named-pipe behavior, effective access control, remote-client rejection, and SCM lifecycle.
- Record the production binary measurement and test behavior-preserving Clap feature reductions against the user-approved 3,700,000-byte Plan 162 ceiling; retain default diagnostics if reductions regress CLI output.

Out of scope:

- Any further increase or re-baseline beyond the user-approved 3,700,000-byte Plan 162 ceiling.
- Treating unresolved test failures as “baseline-only” without a reproduced cause and evidence.
- Changing production Unix socket fallback behavior unless a separate product defect is reproduced independently of the fixture path-length error.
- Allowing unit tests to read, truncate, or remove the real `/var/log/greggd.log`.
- Weakening the named-pipe SDDL, `PIPE_REJECT_REMOTE_CLIENTS`, control framing, deadlines, SCM ownership checks, or existing startup/uninstall safety contracts.
- Treating a Windows GNU cross-check as native Windows qualification.
- Closing or reclassifying Plan 162, 196, or 197 merely because Plan 198 is implemented, or because this plan has been written.

## Workstreams

### 1. Reproduce and repair Unix control-socket fixtures

1. Re-run the eight previously failing Unix socket tests individually and together; preserve their names and actual failure output. Confirm the relevant path budget on the active platform (macOS `sun_path` is 104 bytes) and record the computed socket pathname, rather than inferring a production defect from a failed `Option`/path assertion.
2. Give tests whose purpose is to exercise the primary socket a deliberately short temporary root with a test helper that guarantees the complete primary pathname, including the socket filename, fits the platform limit. Assert that the primary candidate was selected.
3. For tests intended to exercise fallback behavior, derive the endpoint through the production primary-or-fallback selection helper and assert the selected fallback path and config identity. Do not independently reimplement path selection in test code.
4. Keep the tests for distinct-config identity, stale cleanup, and control protocol semantics meaningful on both the primary and fallback paths. Do not change production path selection merely to make a long test fixture pass.
5. Re-run all eight tests and the complete `greggd` test suite; retain the test names and final counts in the evidence record.

### 2. Isolate purge tests and make preflight complete

1. Refactor the uninstall cleanup boundary to accept an explicit log-path dependency (or an equivalent private injected path set). Production continues to resolve the same canonical log path; unit tests supply a file under their own temporary directory. Prefer explicit dependency injection over process-global environment mutation, which can race parallel tests.
2. Ensure `--purge` preflight checks the writable/usable parent of every component path it will remove, including the log path, before any service artifact or config teardown mutation. Keep non-purge behavior unchanged and preserve the existing component-only deletion boundary.
3. Add tests proving that purge uses and removes only the injected temporary log, that ordinary uninstall preserves it, and that an unremovable/unusable log parent is rejected before any service/config mutation. Tests must not invoke the destructive cleanup path with `/var/log/greggd.log`.
4. Review all error paths so the test seam cannot alter production path resolution and a failed preflight leaves service artifacts, config, and log untouched.

### 3. Close Plan 196’s atomic-writer outcome edge

1. Inspect `Config::write_atomic` and its callers. Move fallible preparation/metadata operations before the atomic replacement where possible.
2. Define and implement a truthful contract for failures after rename, especially parent-directory synchronization: either make the operation report a distinct committed-but-durability-uncertain outcome, or otherwise ensure callers can distinguish “not committed” from “replacement occurred but durability confirmation failed.” Do not claim that the original bytes are preserved after a post-rename error, and do not attempt a rollback that weakens atomicity.
3. Add deterministic fault-injection tests for failures before rename and after rename. Before-commit failures must preserve the original bytes; after-rename failures must report the committed/uncertain state accurately and callers must not describe the old config as retained.
4. Update affected CLI diagnostics and user-facing documentation if the observable write result changes. Follow the repository requirement to update `README.md`, `crates/greggd/README.md`, the relevant architecture/skill documentation, and `CHANGELOG.md` for any user-visible behavior change. Keep the Plan 196 historical record intact and append its closure evidence only after its own criteria pass.

### 4. Clear macOS FFI strict-Clippy findings

1. Re-run the exact strict Clippy command on the macOS host and enumerate the current diagnostics; the known count is 15 `borrow_as_ptr` and one `too_many_lines` in `crates/gregg-host/src/macos/ffi.rs`.
2. Replace the raw-pointer borrows with the appropriate raw-address/pointer expressions while preserving initialization, aliasing, and safety-comment requirements. Refactor the long `disk_io` routine into cohesive private helpers if needed; do not silence the warning with a module-wide or unexplained `allow`.
3. Preserve the native Mach/FFI behavior and existing test coverage. Run macOS `gregg-host` tests, `greggd` macOS adapter tests, and strict Clippy after the changes; verify there are no new warnings.

### 5. Complete local test and lint gates

Run these to completion after the fixes, recording exact command, exit status, and test totals:

~~~sh
cargo fmt --all -- --check
cargo test -p greggd --all-targets --all-features
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

The interrupted local-check attempt is not evidence of a pass. If the repository script overlaps another gate, still complete the script itself because its recorded result was interrupted. Investigate and fix every failure in the applicable gates; do not relabel a failure as baseline-only. Include focused regressions for the socket fixtures, purge isolation/preflight, and atomic-writer outcomes in the evidence record.

### 6. Native Windows transport, ACL, remote-client, and SCM qualification

1. Run the existing native Windows CI job on the corrected source. Its full-workspace strict Clippy, full-workspace tests, release builds of `greggd` and `gregg`, and `scripts/smoke-windows.ps1` SCM lifecycle smoke are the platform authority. Record the exact CI run and commit. A GNU cross-check may be recorded as development evidence only.
2. Extend native tests beyond checking the SDDL string: inspect or exercise the effective pipe access descriptor and prove the intended local identity is allowed while a non-owner principal/token is denied, using an access-check or connection-level test appropriate to the Windows runner. Keep the owner-only contract unchanged.
3. Prove `PIPE_REJECT_REMOTE_CLIENTS` behavior with an actual remote named-pipe connection attempt from a second Windows host or an existing trusted Windows integration environment, and record the rejected result. A local client test or source inspection of the flag is not remote-client evidence. If the existing hosted runner cannot provide a remote host, retain this criterion as blocked until the documented native two-host qualification is run; do not substitute a cross-compile.
4. Exercise the path-free bounded `REHASH` and `CONFIGTEST` requests and their acknowledgments on the native pipe while confirming existing `STOP` behavior, deadlines, and SCM ownership/lifecycle behavior remain intact. Do not add a new workflow/job/matrix unless the existing CI cannot execute a required native test and a separately justified plan authorizes it.

### 7. Attribute and remediate the binary footprint

1. Apply Plan 162’s additive user-approved amendment: a stripped Darwin arm64 release `greggd` must be at or below **3,700,000 bytes**. Preserve the original 3,400,000-byte text as historical evidence; do not substitute `cargo bloat` output for the production artifact measurement.
2. Record the target, Rust/toolchain, release profile settings (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, and the actual panic strategy), exact source revision/tree, build command, and byte-measurement command. Build the normal production release artifact for `aarch64-apple-darwin` and measure that artifact. At the current source revision, `Cargo.toml` deliberately leaves panic unwinding enabled because production panic supervision depends on it; do not override to `panic = "abort"` just to reduce size. Plan 162's historical panic-abort setting is not permission to remove those runtime guarantees. The previously recorded snapshots—Plan 196: 3,472,944; Plan 197: 3,539,152; Plan 198: 3,555,712 bytes—are comparison points, not a component attribution or a new baseline.
3. Use matched-source ablations or equivalent repeatable component measurements to attribute increases across the scheduled-job CLI/editor, local rehash/configtest transport, and other relevant changes. `cargo bloat` may guide investigation, but its unstripped analysis artifact is not the size gate; record that distinction explicitly.
4. Apply only evidence-supported reductions that preserve the CLI, scheduler, control security, diagnostics, and native platform contracts. Do not add a dependency or remove required behavior merely to hit the number without documenting the tradeoff and obtaining a separate scope decision.
5. Rebuild and measure after the final source change. If the binary exceeds 3,700,000 bytes, acceptance stays blocked. Do not raise or re-baseline the amended ceiling; any future code-size reduction that changes required behavior needs a separately scoped implementation decision.

## Acceptance criteria

- [x] All eight formerly failing Unix control-socket tests pass with fixtures that deliberately select the primary or fallback path; production fallback behavior was not changed without an independently reproduced defect.
- [x] The complete `greggd` test suite and workspace test suite pass with zero failures; failures are not dismissed as baseline-only.
- [x] Purge tests use only injected temporary paths; the real `/var/log/greggd.log` is never touched by a unit test; purge preflight covers the log path and rejects before teardown on failure.
- [x] Plan 196’s writer has a tested, truthful pre-/post-rename error contract, and callers preserve that distinction.
- [x] The 16 known macOS FFI Clippy findings are resolved without broad lint suppression; native macOS tests and workspace strict Clippy pass.
- [x] `cargo fmt --all -- --check` and a complete `./scripts/check-local.sh` run pass; no interrupted or partial run is counted.
- [ ] Native Windows CI passes its full-workspace Clippy/tests, release builds, and SCM smoke; effective local pipe ACL enforcement and actual remote-client rejection have evidence from native Windows execution. (The CI gates and the effective ACL are evidenced at `62d78f5`; remote-client rejection still needs a second Windows host.)
- [x] The production-profile stripped Darwin arm64 `greggd` measures no more than the amended 3,700,000-byte ceiling; the exact artifact measurement and Clap feature/behavior tradeoffs are recorded in the 2026-11-25 follow-up below.
- [x] Plans 196, 197, and 198 retain separate acceptance statuses. Plan 162's original record and historical acceptance status are preserved, with the approved prospective ceiling amendment documented additively; Plan 198 is not used to imply closure of Plan 196 or 197.

## Stop conditions and evidence rules

- If a test failure cannot be reproduced or its cause cannot be established, keep it unresolved and acceptance blocked; do not classify it as baseline-only.
- If any proposed uninstall change could mutate service/config state before all purge targets pass preflight, stop and correct the ordering before destructive qualification.
- If native Windows remote-client rejection cannot be exercised, record the missing environment and leave the Windows qualification criterion open.
- If the production artifact exceeds the amended 3,700,000-byte ceiling, keep footprint acceptance blocked and request a separately scoped policy or behavior decision; do not remove required CLI, scheduler, security, diagnostic, or platform behavior to force a pass.
- Keep separate evidence summaries for Plans 196, 197, 198, and 199. Shared passing gates may be cross-referenced, but each earlier plan’s status changes only when its own remaining acceptance criteria are proven.

## References

- Plan 162 footprint contract: `plans/162-scheduler-observability-contract-and-resource-qualification.md`.
- Plan 196 writer and CLI acceptance: `plans/196-greggd-scheduled-job-cli-and-config-editor.md`; `crates/greggd/src/config.rs`.
- Plan 197 rehash/control acceptance: `plans/197-greggd-rehash-live-scheduled-jobs.md`; `crates/greggd/src/control.rs`; `crates/greggd/src/control/windows_pipe.rs`.
- Plan 198 current implementation/evidence: `plans/198-greggd-job-auto-rehash-and-configtest.md`.
- Purge ordering and cleanup: `crates/greggd/src/uninstall.rs`.
- macOS FFI Clippy diagnostics: `crates/gregg-host/src/macos/ffi.rs`.
- Native Windows CI and SCM smoke: `.github/workflows/ci.yml`; `scripts/smoke-windows.ps1`.

## Plan 199 follow-up evidence (2026-11-24)

### Local corrective qualification

- `cargo test -p greggd --all-targets --all-features`: passed, 582 library tests, 3 binary tests, and 2 integration tests (587 passed; 0 failed). The corrected socket and purge regressions ran in this suite.
- `cargo test --workspace --all-targets --all-features`: passed, 1,878 tests passed, 2 ignored, 0 failed.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and a complete `./scripts/check-local.sh`: all exited 0; the local script reported `all checks passed (mode: default)`.
- Native macOS `gregg-host` tests: 64 passed. `bash scripts/tests/test-install-rerun.sh`: 112 checks passed. No native Windows CI, effective ACL test, remote-client attempt, or SCM run was available from this macOS worktree.

### Fixed-ceiling footprint result

- Target: `aarch64-apple-darwin`; `rustc 1.100.0 (0ef1bc386 2026-11-09)`, LLVM 23.1.5. Release settings are `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`; the current `Cargo.toml` has no `panic` override and retains unwind behavior. It was not built with `panic = "abort"`.
- Current source: HEAD `b2792ac1c6be6c5a78a03926d25e57fbe32b8257` plus the uncommitted implementation tree; relevant-input diff digest (Cargo manifests/lockfile, `greggd`, and macOS FFI) `e665257a9ec9ef8ced1affbe0326623e24966e02edb30451046f3e6757a75e2b`.
- Production command: `cargo build --release --target aarch64-apple-darwin -p greggd`; measurement: `stat -f '%z bytes' target/aarch64-apple-darwin/release/greggd`. A fresh rebuild of the current tree measured **3,555,712 bytes**, **155,712 bytes over** Plan 162's unchanged 3,400,000-byte ceiling.
- Matched source-stage builds used the same Rust toolchain, current workspace release profile, and current lockfile: immediate pre-Plan-196 commit `2ade839659ccafff63fb119d99c3588714a3ba4f` measured **3,356,832 bytes**; Plan 196 commit `2df76255c9f49a3d465294dd9fd90b7a706dac55` measured **3,472,944 bytes**. Plan 196 therefore adds **116,112 bytes** in this paired measurement and is already **72,944 bytes over** the fixed ceiling before Plans 197 and 198.
- Current Plan 197/198/199 source measures **82,768 bytes above** the matched Plan 196 source. Recorded intermediate Plan 197 and Plan 198 artifacts are 3,539,152 and 3,555,712 bytes (+66,208 and +16,560 sequentially); these are staged comparisons, not separate matched-source ablations. The Plan 197/198 component split is not fully attributed.
- No behavior-preserving reduction sufficient to meet the ceiling was identified; no required CLI, scheduler, security, diagnostic, or native-platform behavior was removed, and no threshold was changed. Footprint acceptance remains open. A separate scoped decision is needed before attempting a substantial reduction that changes or compromises the required behavior; Plan 199 does not authorize a re-baseline.

Plan 199 remains in progress. Plans 162, 196, 197, and 198 retain their separate acceptance statuses.

## User-approved footprint amendment and Clap feature study (2026-11-25)

The original Plan 162 3,400,000-byte limit is retained above as historical
contract text. At the user's direction, the current prospective blocking ceiling
is **3,700,000 bytes**, inclusive. The standard restored-default Clap release
build measured **3,555,696 bytes**, 144,304 bytes below that ceiling. Build
command: `CARGO_TARGET_DIR=target/clap-default cargo build --release --target aarch64-apple-darwin -p greggd`; measurement: `stat -f '%z bytes' target/clap-default/aarch64-apple-darwin/release/greggd`. This is a threshold amendment, not a code or release-profile change. The repository's blocking footprint criterion is documented in plans; no executable size check was found under `scripts/`.

Clap optional-feature ablations were measured from the same release target:

| Configuration | Size | Delta from defaults | CLI result |
|---|---:|---:|---|
| Clap defaults (`derive` in manifest) | 3,555,696 B | — | Preserves existing diagnostics |
| Remove `suggestions` | 3,539,152 B | −16,544 B | Loses the similar-subcommand hint |
| Also remove `color` | 3,522,608 B | −33,088 B | No additional diagnostic change beyond color |
| Also remove `error-context` | 3,489,552 B | −66,144 B | Unknown-command output loses the offending input, usage, and help hint |

None of the tested feature sets met the former 3,400,000-byte limit. The
smallest tested set passed the `greggd` test suite (587 passed) and help smoke
test, but the invalid-command diagnostic regression is observable. The manifest
was restored to Clap defaults; no feature reduction was retained. The selected
3,700,000-byte limit accommodates the measured production build without
removing these CLI diagnostics.

## Effective ACL qualification and the remaining remote-client item (2026-11-25)

- Native Windows now evidences effective access control rather than the SDDL
  string. `created_pipe_object_carries_the_protected_dacl` opens the created
  pipe and reads its attached security descriptor, which must be a protected
  DACL granting only Administrators and LocalService (the round-trip renders the
  generic full-access ACE as the concrete `FA` mask, so both canonical
  spellings are accepted). `a_deny_only_administrators_token_cannot_connect`
  impersonates a restricted token whose Administrators group is deny-only and
  asserts the connect fails with `ERROR_ACCESS_DENIED`, which exercises the
  access check instead of inspecting the policy text.
- Hosted CI run `45725598320` at `62d78f5` is green across all seven jobs.
  The native Windows job passed Clippy, the full-workspace test suite, both
  release builds, and the SCM lifecycle smoke, so the CI portion of the open
  criterion is satisfied.
- The remote-client half is not satisfied and is not substituted. It requires an
  actual connection attempt from a second Windows host or an existing trusted
  Windows integration environment, per this plan's stop condition; neither this
  macOS worktree nor the single hosted runner can provide that. The criterion
  therefore stays open with only that item outstanding.
- Two unrelated corrections were needed to obtain a green run: the ACL probe
  had to create its pipe inside a tokio runtime and read the DACL to its NUL
  terminator, and `ui::cron::tests::clock_skew_degrades_to_zero_rather_than_wrapping`
  pinned an exact second against a clock read taken after `now`, so a
  millisecond of delay rendered `1m29s`. Both are recorded here rather than
  dismissed as baseline noise: the first was this plan's own new test, and the
  second now accepts the same adjacent readings its neighbouring assertion
  already accepted.

### Remote-client rejection: hosted-CI feasibility (2026-11-26)

Checked whether the existing hosted workflow can supply the remote client this
criterion requires, since the stop condition below permits only native Windows
execution.

- Each GitHub-hosted runner is a new, isolated virtual machine with outbound
  network access and no inbound path, so one job cannot dial a pipe hosted by
  another job.
- The hosted Windows images offer no nested virtualization, so a job cannot
  start a second Windows guest to act as the remote client.
- Windows containers on the runner share the host kernel and are not a second
  host; a connection to the host's own `\\.\pipe\` name is local by
  construction.
- Runner private networking exists only as an Azure VNET feature for
  organization-level hosted runners, which this repository does not use.
- Connecting to the runner's own machine name would still not be a second host,
  so even a rejection observed there could not meet the criterion as written.

Conclusion: hosted CI cannot close this item. The criterion stays blocked
exactly as the stop condition directs. A documented two-host qualification on
two Windows machines that can reach each other is the only path under the
current standard; accepting same-host SMB-origin evidence instead would be an
explicit amendment this plan does not authorize on its own.
