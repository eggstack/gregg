# Plan 197: local greggd rehash for scheduled jobs

Status: implemented; acceptance remains blocked on suite/workspace-Clippy, native Windows, and footprint qualification.

Source: user request for an explicit `greggd rehash` command that reloads configuration—especially scheduled jobs—into a running daemon without restarting it. The user explicitly accepts host/port changes as restart-required.

Depends on: the existing scheduled-job configuration and scheduler from Plans 157-163, and the current post-196 code line for daemon config/CLI conventions. This plan does not depend on or close Plan 196's outstanding acceptance blockers; preserve its current status and evidence unchanged.

## Objective

Add a local-only `greggd rehash` command that asks the already-running daemon, identified by the selected config path, to read and validate that config and reconcile its scheduled-job set in place. The daemon must acknowledge only after the scheduler has applied the accepted job set. No process, scheduler-task, or HTTP-server restart is required, and no existing child is killed.

The first version deliberately supports live changes to `jobs` only. It reads and validates the complete config, but changes to any other config field are restart-only. A candidate that changes both `jobs` and a restart-only field is rejected atomically: apply none of it, report the changed restart-only fields, and leave the current scheduler untouched. This avoids claiming that the daemon has fully reloaded a config while silently retaining a mixture of old and new values.

## User-facing contract

    greggd [--config PATH] rehash

Examples:

    greggd rehash
    greggd --config /etc/gregg/greggd.toml rehash

- The selected config path is used only to resolve the same config-specific local control endpoint used by the daemon. The client sends a fixed `REHASH\n` request; the daemon reads its own captured config path. The request cannot name an arbitrary file.
- The daemon reads one config snapshot, runs the existing complete config validation and the existing root-job authority check, then compares it with the active config. A valid job-only change is applied to the live scheduler before the CLI receives `APPLIED`. A valid no-op returns `UNCHANGED`. Malformed/invalid config, authority failure, restart-only diffs, scheduler refusal, and transport uncertainty are distinct outcomes; none may be reported as applied.
- Restart-only fields in this version are `name`, `host`, `port`, `sample_interval_ms`, `stale_after_ms`, `allow_privileged_jobs`, and `scheduler_history_limit`. In particular, host/port remain bound at startup. Future fields are restart-only until deliberately added to the live-apply contract and tested.
- The operation is all-or-nothing. If any restart-only field differs from the daemon's active config, return `RESTART_REQUIRED` with the changed field names and do not apply job changes from that candidate. A rejected config never changes the last-known-good scheduler or active-config baseline.
- Repeating `rehash` is safe: unchanged jobs do not reset deadlines/pending work, start duplicate children, clear history, or create an immediate run. New/changed config does not mean `run now`; new jobs become eligible on their next scheduled occurrence.
- This command does not start a missing daemon, invoke a service manager, stop/restart a process, watch the config file, or mutate the HTTP API. After `greggd job add/remove/edit` or a manual edit, the operator explicitly runs `greggd rehash`; otherwise the edit remains on disk until daemon restart.
- A response timeout after request delivery is `UNCERTAIN`, not success or proof of rejection: the daemon may have applied it before the reply was lost. Do not automatically retry or hide this distinction. The command remains safe to invoke again because the operation is idempotent.
- The HTTP scheduler summary/history endpoints remain read-only and unchanged. No scheduler mutation route, protocol schema change, or new network exposure is introduced.

The local control protocol uses bounded, newline-terminated requests and responses. Unix extends the existing config-specific socket while preserving `STOP\n` behavior and its restrictive `0600` permissions. Windows needs an equivalent config-specific local endpoint with an explicit, tested ACL appropriate to the installed service identity and authorized operator; an SCM control acknowledgment alone is insufficient because it does not prove that reconciliation completed. Both platforms reject malformed/oversized frames, bound I/O and the apply acknowledgment by fixed deadlines, and fail closed if a secure endpoint cannot be established. Keep responses small and machine-distinguishable (`APPLIED`, `UNCHANGED`, `RESTART_REQUIRED`, `REJECTED`, `BUSY/UNCERTAIN`); cap any diagnostic text.

## Scope

In scope:

- Add `rehash` to the `greggd` CLI, preserving global `--config` selection, stable per-config identity, local-only behavior, and truthful result/exit classification.
- Extend Unix control handling and add the Windows-local control path so the CLI receives the scheduler's applied/rejected result rather than merely confirmation that a request was queued.
- Add one daemon-owned config reload coordinator that validates the candidate, enforces the restart-only field boundary and job authority, and sends a bounded reconciliation request to the existing scheduler worker.
- Keep one scheduler worker alive for the daemon lifetime even when the initial job list is empty. It must accept the first job set, transitions to/from an empty set, and later rehash requests without recreating the task.
- Reconcile jobs in place without replacing the engine's history publisher or weakening the one-global-child invariant. Refactor the borrowed config/engine ownership only as required to support reconciliation.
- Document the changed config lifecycle in `README.md`, `crates/greggd/README.md`, `architecture/greggd-daemon.md`, `.opencode/skills/greggd-daemon/SKILL.md`, `CHANGELOG.md`, CLI help, and this plan index.

Preserved exclusions:

- No live host/port rebinding, listener/server restart, general reload of sampler cadence/stale policy/name/privilege/history settings, or partial application of a mixed candidate.
- No config watcher, automatic rehash from `job add/remove/edit`, implicit daemon restart, process discovery, PID file, shell, `sudo`, or service-manager mutation.
- No HTTP/API scheduler mutation, new protocol schema/version, client/TUI change, new job editing semantics, run-now, or scheduler cadence redesign.
- No scheduler-task replacement as a reload mechanism: its shutdown path can kill an active child. No child cancellation, concurrent second scheduler, unbounded command queue, or silent dropped control message.
- No weakening of full config validation, direct-argv execution, same-principal job execution, root opt-in, service sandboxing, read-only scheduler API, or bounded in-memory history.
- No unrelated cleanup and no new CI workflow/job/matrix. Add dependencies only if the existing platform APIs cannot satisfy the contract and the dependency/MSRV/footprint cost is justified.
- Plan 196 remains acceptance-blocked exactly as recorded in `plans/README.md`. Plan 197 does not resolve its post-rename writer edge, unrelated suite failures, missing native CI, or binary-size overage. Do not raise Plan 162's 3,400,000-byte stripped-`greggd` ceiling; report the measured size and keep acceptance blocked if it is not met.

## Phase 1: request/acknowledgment and reload policy

1. Add `rehash` to the `greggd` command model and help. Resolve the control target from the selected config path exactly as `stop` does; do not read, rewrite, or pass a path to the remote daemon in the request.
2. Define typed rehash results for `Applied`, `Unchanged`, `RestartRequired(fields)`, `Rejected(reason)`, and transport/server uncertainty. Map them to truthful CLI output and nonzero status for every non-applied result; retain the existing command exit-code conventions.
3. Extend Unix local control with `REHASH\n` while keeping `STOP\n` wire behavior compatible. Add a Windows-local endpoint with a config-specific identity and an ACL tested against the actual service principal/authorized operator. Do not treat SCM acceptance as the applied acknowledgment.
4. Bound request size, response size, per-client handling, config load/apply wait, and client wait. The server's apply deadline must be shorter than the client's wait; if the client times out after delivery, say the outcome is uncertain. A full bounded internal channel returns a visible busy/uncertain result rather than dropping or endlessly queueing the request.
5. On the daemon side, reload only the config captured for that daemon. Parse and validate the entire candidate and repeat `validate_job_authority` using the current platform privilege check before any scheduler mutation. Compare all non-`jobs` fields to the active config snapshot; on any difference, return `RestartRequired` and leave all live state unchanged. A validation/authority error similarly leaves the active config and scheduler untouched.
6. Only return `Applied` after the scheduler has acknowledged reconciliation. Diagnostics are bounded; no secret values, command output, or unbounded config text are logged or sent over the control connection.

## Phase 2: persistent in-place scheduler reconciliation

1. Make the scheduler controller/task daemon-lifetime rather than conditional on `jobs.is_empty()`. Keep the empty scheduler document valid from startup, provide the load watch regardless of initial job count, and allow the first accepted rehash to construct live state without restarting the daemon.
2. Add a bounded scheduler reconciliation channel with per-request completion acknowledgment. Keep config processing serialized and correlate each reply to its request. The scheduler checks for a reload while idle and while waiting for an active child; an accepted request must not wait indefinitely for that child to exit.
3. Reconcile by stable job name and full job definition: preserve schedule/pending/retry state for unchanged definitions; initialize new jobs at the next occurrence strictly after reconciliation time; clear obsolete pending state for changed or removed jobs; never execute a new or edited job immediately just because rehash ran. Publish the accepted current job set through the existing publisher.
4. Preserve the current active child unchanged across reconciliation. The child remains the sole global execution slot until its direct process completes, even if its job was removed or edited. Freeze the launched job identity/name on the active run so completion/output history is attributed to the configuration that actually launched it, not a new vector index or a different same-name definition. Do not cancel or relaunch it.
5. Preserve the existing `SchedulerObserver` and bounded history across all job-set changes, including transitions to zero jobs. Removing/replacing the engine must not clear retained records or reset the publisher epoch/revision in a way that misattributes results. Keep history caps, output caps, read-only HTTP routes, ordering, and one-child scheduling behavior unchanged.
6. No-op rehashes must not publish a spurious operator-visible update or reset unchanged deadlines. Successful changes publish the reconciled live summary before the acknowledgment; active-child status remains accurate during and after the reload.
7. Keep scheduler fatal errors under current daemon supervision. A bad candidate/control request is a rejected operation, not a daemon failure; an actual worker failure remains supervised and must not be disguised as an empty schedule.

## Phase 3: regression coverage, documentation, and qualification

1. Add deterministic config-policy tests for job-only acceptance; each restart-only field (including host and port); mixed job plus restart-only changes; invalid TOML/schema; root-job opt-in failure; and last-known-good preservation. Assert no partial mutation on every rejection.
2. Add Unix control tests for `REHASH` success only after apply, unchanged/rejected/restart-required replies, malformed/oversized requests, bounded silent clients, and compatibility with `STOP`. Add native Windows pipe tests for the same outcomes and allowed/denied ACL access; Windows CI is authoritative for the `cfg(windows)` implementation.
3. Add CLI tests for global `--config` targeting, no-running-daemon behavior, each response and exit classification, and no service-manager/process-spawn fallback. Test that a timeout is shown as uncertain and is not claimed as rejection or success.
4. Add scheduler tests with injected clocks/paused time and controlled child processes: initially empty to first job; add/change/remove; transition to zero and back; unchanged-job pending/retry preservation; changed-job pending reset; no immediate execution; history retention; rehash during a long-running child; child completion after removal/edit attributed to the original job; and no second child while it remains active.
5. Verify the existing read-only scheduler API still publishes an empty/live summary and retained history coherently across rehash, with no protocol changes, duplicate records, fabricated outcomes, or publication on an unchanged rehash.
6. Update user docs/help and the daemon skill with exact CLI syntax, local-only behavior, job-only live-apply scope, restart-required fields (host/port included), all-or-nothing mixed-change behavior, acknowledgment/uncertain outcomes, root opt-in, and the explicit rehash step after editing. Update the changelog. Do not rewrite Plan 196's blocked implementation/verification record as complete.
7. Run focused `greggd` tests, `cargo fmt --all -- --check`, `cargo clippy -p greggd --all-targets --all-features -- -D warnings`, and `./scripts/check-local.sh`; run the existing native platform CI, especially Windows and macOS. Do not add CI infrastructure. Record any unrelated baseline failures separately and never call them passing.
8. Measure stripped `greggd` against Plan 162's unchanged 3,400,000-byte ceiling. The post-196 binary was measured at 3,472,944 bytes, already 72,944 bytes over the limit; Plan 197 cannot close this criterion or claim Plan 196's blocker resolved by merely reporting a small/zero code-size delta. A separate accepted size correction (or an explicit user-approved Plan 162 policy change) is required before a passing footprint claim.

## Acceptance criteria

- [ ] `greggd [--config PATH] rehash` sends one bounded local request and distinguishes `APPLIED`, `UNCHANGED`, `RESTART_REQUIRED`, `REJECTED`, and uncertain/not-running outcomes truthfully.
- [ ] Unix local control remains mode `0600`, config-specific, bounded, and STOP-compatible; Windows has an equivalent config-specific local path with a verified restrictive ACL and applied-result acknowledgment.
- [ ] The daemon loads only its own config path, validates the full candidate and root-job authority, and applies only job-only diffs. Every restart-only diff (host/port, name, cadence, stale policy, root opt-in, retention, and future unclassified fields) rejects the entire candidate without changing runtime state.
- [ ] Invalid config, authority failure, unsupported diff, busy channel, or control failure never changes the last-known-good active config or live scheduler; the CLI never reports a queued request as applied.
- [ ] The scheduler worker accepts the first job set when startup had no jobs, plus transitions to/from zero jobs, without daemon or scheduler-task restart.
- [ ] Job reconciliation preserves unchanged pending/retry state, resets pending state for changed/removed jobs, starts new jobs only at future schedule occurrences, and publishes no duplicate/immediate execution on rehash.
- [ ] A running child is never killed or duplicated by rehash; it remains the one active slot, and its final record/output remains attributed to the job definition that launched it.
- [ ] Existing bounded scheduler history and read-only API remain coherent and retained across accepted job-set reconciliation; no wire/schema/API mutation change is added.
- [ ] Existing direct argv, same-principal execution, root opt-in, scheduler cadence/load-gate/one-child limit, service sandboxing, and shutdown behavior remain intact.
- [ ] Docs/help/changelog and the daemon skill describe the implemented job-only reload contract and restart boundary; `job add/remove/edit` never silently restarts or implicitly rehashes.
- [ ] Focused tests, local checks, and existing native platform CI pass or are reported as outstanding; the stripped binary meets Plan 162's unchanged 3,400,000-byte ceiling. Plan 196's blockers remain separately recorded until independently resolved.

## Risks and stop conditions

- Restarting the current scheduler task is unsafe because shutdown kills its active child. Any design that tears down/recreates the task or cancels a child to reload stops this plan.
- The current scheduler engine borrows a fixed job slice and its run loop awaits child completion. Reconciliation therefore has to preserve one engine/observer/history while making reload selectable during that wait; queueing changes until the child exits is not acceptable.
- New jobs must work when there were no jobs at startup. Do not treat an empty initial set as permission to omit the control/scheduler lifetime needed for later activation.
- Ambiguous mixed configuration changes could leave operators believing host/port or other startup-bound settings changed. Keep v1 all-or-nothing, list restart-only fields, and never imply the listener moved.
- A process/service restart, HTTP mutation route, watcher, or scheduler result-history reset is not an alternate implementation of rehash.
- The current post-196 stripped daemon already exceeds the Plan-162 ceiling and Plan 196 also has independent open acceptance items. Report these as blockers; do not waive the ceiling, rewrite Plan 196's status, or count Plan 197 as resolving them.

## References

- Config fields, complete validation, and root-job authority: `crates/greggd/src/config.rs`.
- Startup-time config capture, conditional scheduler creation, and daemon supervision: `crates/greggd/src/run.rs`.
- Fixed job-vector engine, active-child wait, and shutdown behavior: `crates/greggd/src/scheduler.rs`.
- Current Unix STOP-only config-specific local control protocol: `crates/greggd/src/control.rs`.
- Windows service dispatch/control lifecycle: `crates/greggd/src/service/windows.rs`.
- Scheduler history and footprint/read-only API contract: Plans 162-163 and `architecture/greggd-daemon.md`.
- Existing job/config CLI and atomic writer (including Plan 196's unresolved post-rename caveat): `crates/greggd/src/cli.rs`, `crates/greggd/src/config.rs`, and Plan 196.

## Implementation and qualification record (2026-11-24)

The implementation is present in the current uncommitted worktree. It adds the bounded local rehash request/acknowledgment path, complete candidate validation and repeated root-job authority validation, restart-only diff rejection, bounded off-runtime config loading, and persistent in-place scheduler reconciliation that preserves the observer/history and any active child. The CLI reports applied, unchanged, restart-required, rejected, not-running, and uncertain outcomes distinctly.

Checks completed on this worktree:

- `cargo test -p greggd --all-targets --all-features rehash`: 12 passed, 0 failed (including the latest unchanged-reconciliation test and bounded-load regression).
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p greggd --all-targets --all-features --no-deps --message-format=short -- -D warnings`: passed.
- Windows GNU target check and isolated Clippy completed as development cross-checks; they do not replace native Windows CI or ACL/service behavior tests.

Acceptance remains blocked and these are not green qualifications:

- `./scripts/check-local.sh` failed with 988 passed and 2 unrelated EggPool tests failed. The full `cargo test -p greggd --all-targets --all-features` run failed 10 tests (8 Unix control-path setup failures and 2 uninstall permission failures involving `/var/log/greggd.log`); it was not rerun after the final test-only addition.
- The full strict Clippy gate reports 16 warnings-as-errors in unchanged `gregg-host/src/macos/ffi.rs`; the isolated `greggd` no-deps gate above passes.
- Native Windows CI, actual allowed/denied named-pipe ACL checks for the service/operator identities, and native SCM behavior remain outstanding.
- The stripped Darwin arm64 release daemon measured 3,539,152 bytes, 139,152 bytes above Plan 162's unchanged 3,400,000-byte ceiling. This ceiling is not waived.

Plan 196's separate acceptance blockers and status are unchanged; Plan 197 does not claim to resolve them. No hosted CI result or commit is claimed.

## Follow-up acceptance checks (2026-11-24, local macOS)

Fresh checks were run against the current uncommitted worktree:

- `./scripts/check-local.sh`: failed at the workspace test stage: 989 passed, 1 failed, 2 ignored. The failing test was `eggpool::tests::a_worker_converges_onto_newer_intent_while_a_result_is_backpressured` (panic: “no EggPool summary request was issued”, `crates/gregg/src/eggpool.rs:1794`).
- `cargo test -p greggd --all-targets --all-features`: 555 passed, 10 failed. Eight Unix control-path tests failed during path/setup assertions; two uninstall purge tests failed because removal of `/var/log/greggd.log` was denied. These failures remain unresolved; this run does not establish that they are baseline-only.
- Native `gregg-host` macOS tests: 58 passed, 0 failed. The attempted `greggd` macOS collector filter matched 0 tests, so it is not counted as a passing daemon-adapter test.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: failed in unchanged `gregg-host/src/macos/ffi.rs` (16 library diagnostics; the test target reported 22 diagnostics). The focused no-deps `greggd` strict Clippy gate passed in the preceding check run.
- `cargo fmt --all -- --check`: passed.
- The aarch64-apple-darwin release build passed. The resulting `target/aarch64-apple-darwin/release/greggd` measured 3,539,152 bytes, still 139,152 bytes over Plan 162's 3,400,000-byte ceiling.
- Windows GNU cross-target check/Clippy remain development-only evidence from the preceding run. Native Windows CI, actual allowed/denied named-pipe ACL checks, and native SCM behavior were not run: this worktree is uncommitted and the available host is macOS.

The acceptance status remains blocked. No suite failure is reclassified as an unrelated baseline without evidence, the Plan 162 limit is unchanged, and Plan 196's independent blockers remain untouched.

## Plan 199 follow-up qualification (2026-11-24)

The final local `greggd` and workspace test suites, formatting, strict workspace Clippy, and complete `check-local` now pass after Plan 199's fixture and lint corrections. Native Windows effective ACL, actual remote-client rejection, and SCM qualification remain unrun from this macOS host. The Plan 197 recorded stage remains 3,539,152 bytes, 139,152 over the fixed ceiling; the fresh current Plan 199 tree is 3,555,712 bytes, 155,712 over. Plan 197 remains blocked and separate from Plans 196 and 198.

## Subsequent footprint-limit amendment (2026-11-25)

Plan 162's original 3,400,000-byte limit remains historical; by explicit user
approval, the current prospective ceiling is 3,700,000 bytes. The recorded Plan
197 stage (3,539,152 bytes) and the current restored-default build (3,555,696
bytes) are both within the amended limit. This changes only the applicable size
criterion; native Windows ACL/remote-client/SCM qualification and the distinct
Plan 196/197/198 acceptance statuses remain as recorded.
