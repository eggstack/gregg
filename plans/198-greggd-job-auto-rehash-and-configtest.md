# Plan 198: scheduled-job auto-rehash and configtest

Status: implementation complete; seven of eight acceptance criteria pass. The remaining criterion is the remote-client half of Windows qualification, which requires a two-host run and cannot be produced from this macOS host or the single hosted runner.

Source: follow-up to the request for daemon job add/remove commands to apply persisted changes by default and for a configtest command that checks rehash loadability without applying changes.

Depends on: the Plan 197 rehash implementation and the Plan 196 scheduled-job CLI/config writer. Plan 197 is implemented in the current worktree but its acceptance remains blocked as recorded in Plan 197 and the index. This plan does not close or reclassify Plan 196 or Plan 197 blockers.

## Objective

After a successful `greggd job add` or `greggd job remove` write, automatically attempt the existing config-specific local rehash so a running daemon applies eligible jobs-only changes without a second operator command. Add `greggd configtest` as a read-only check that validates the same config-loading rules as rehash and, when possible, asks the running daemon whether the candidate passes its live rehash policy. It must never write configuration or mutate scheduler state.

The saved config remains authoritative on disk if a rehash attempt is rejected, unavailable, or uncertain. The CLI must distinguish persistence from runtime application and must not imply that a rejected rehash rolled back the file.

## Scope

In scope:

- Auto-attempt rehash after a successful, actual `greggd job add` or `greggd job remove` mutation, including a completed interactive removal.
- Add the `greggd configtest` command, respecting global `--config` selection and rehash's bounded loader semantics.
- Add a bounded, local-only, no-apply config-test request through the existing Unix control socket and Windows named-pipe transport so a running daemon can check daemon-specific eligibility.
- Add truthful status/exit handling, tests, CLI help, and user-facing documentation for both behaviors.

Explicitly out of scope:

- `gregg add/remove` edits the client fleet config, not the daemon scheduler config. Those commands already nudge the per-config Gregg client daemon; they must not call `greggd rehash`.
- `greggd host` and `greggd port` remain restart-required and must not auto-rehash. `greggd edit` retains its current explicit-rehash behavior in this plan; broadening the trigger to the full editor is a separate decision.
- No automatic rehash for list/no-op/cancelled/failed mutations; no watcher, subprocess invocation, service-manager operation, process restart, HTTP mutation route, or network exposure.
- No rollback of a successfully written config when live rehash fails. Do not broaden this plan to resolve Plan 196's post-rename writer-error edge; if the write operation reports failure, do not infer whether it committed and do not send a rehash on that error path.
- Do not change Plan 162's 3,400,000-byte stripped-greggd ceiling or claim this plan resolves Plan 196/197 test, Clippy, native-Windows, ACL/SCM, or footprint blockers.
- No new dependency, CI workflow/job/matrix, wire-protocol schema, scheduler behavior, or read-only HTTP API change.

## User-visible contract

### Job mutation auto-rehash

- Preserve the existing load → mutate → validate → atomic-write flow for `job add/remove`. Only after that call returns success, and after its config lock has been dropped, send one request through the existing platform rehash transport using the selected config path. The control request continues to identify the daemon by config path; it does not carry an arbitrary config path or contents.
- Rehash itself remains the authority: it reloads the final persisted file with the 1 MiB cap, performs complete validation and the daemon-principal job-authority check, rejects restart-only diffs atomically, and acknowledges only after scheduler reconciliation. Do not add a second CLI-only interpretation of rehash eligibility.
- Do not send a request for a duplicate add, invalid add, missing named job, empty job list, cancelled selector, or any other path that did not successfully change the config. `job list` never sends one.
- If the write succeeds but no daemon is running, report that the config was saved and will be loaded on the next daemon start; preserve the existing successful mutation exit status. If a daemon responds with restart-required, rejection, busy, or uncertain, report that the file is saved but not confirmed live-applied and return the existing corresponding nonzero config/runtime class. Never retry automatically after an uncertain response.
- `APPLIED` and `UNCHANGED` are successful post-write outcomes. A response timeout remains uncertain: the saved config stays in place and the daemon may or may not have applied it. Do not roll back, start a daemon, or claim either outcome without evidence.
- Keep explicit `greggd rehash` behavior and its existing result classification unchanged. Keep `gregg add/remove` client-daemon notification behavior unchanged.

### Configtest

- Syntax: `greggd [--config PATH] configtest`. The selected path is used for local file loading and to select the matching local daemon control endpoint. An absent config file is not replaced with implicit defaults: rehash itself needs a readable file to load.
- Use the same `Config::load_for_rehash` bound and the same complete parse/schema validation used by rehash. If local validation fails, report a config failure and return the config exit class. Do not print command contents or secrets.
- When the matching daemon is reachable, send a fixed `CONFIGTEST\n` request. The daemon reads only its captured config path, repeats validation and its own job-authority check, and compares restart-only fields against its current active config. Return a bounded result such as `READY`, `UNCHANGED`, `RESTART_REQUIRED`, `REJECTED`, `BUSY`, or `UNCERTAIN`; do not return `APPLIED` for a test.
- The daemon-side test path must stop before `Engine::reconcile`, active-config replacement, scheduler publication, history changes, child changes, or any config write. It checks loader/authority/restart-boundary eligibility; it does not guarantee a future apply if the file or daemon state changes, or if reconciliation later fails.
- If no daemon is running, `configtest` may still report that the file is load-valid from the local shared loader, but its output must explicitly say that daemon-specific authority and restart-only compatibility were not checked. Do not label this result `READY` or claim it can be applied live. A successful local-only validation returns success; transport/control errors other than plain not-running remain nonzero.
- A live restart-required or authority rejection is a config failure; busy/uncertain/control failures are runtime failures. Diagnostics remain bounded and must not include arbitrary TOML contents.
- Keep the HTTP scheduler API read-only. Unix `STOP\n` behavior, socket permissions/identity, Windows pipe ACL and remote-client rejection remain unchanged; extend request decoding without weakening existing framing or access control.

## Implementation phases

### Phase 1: post-write job rehash

1. Keep `mutate_config_with_result` responsible only for the current locked validation/write transaction. Trigger rehash from the successful `JobCommand::Add` and actual `JobCommand::Remove` dispatch paths after the mutation helper returns, not from inside the lock or from a general config-mutator hook.
2. Reuse the existing Unix/Windows local rehash clients directly; do not spawn `greggd rehash`, shell out, or invoke service management. Factor a small platform helper only if needed to avoid duplicating transport selection at the CLI boundary.
3. Add a post-write reporting path that clearly states both the on-disk mutation and its live result. Keep exit classification above: missing daemon is a deferred successful config mutation; any live rejection or uncertain outcome is a nonzero result with no implication of rollback.
4. Inject or wrap the sender in tests so command behavior can be checked deterministically without a live daemon. Prove no request is sent from failed/no-op/cancelled paths and exactly one request is sent after each successful add/remove.

### Phase 2: no-apply configtest

1. Add `Command::Configtest` and binary dispatch, preserving global config path intent and existing exit-code conventions.
2. Share the rehash loader and candidate-policy checks rather than copying validation logic. Refactor the scheduler/coordinator request path only as required to distinguish apply from check-only while retaining the current bounded single-flight load gate and request queue.
3. Extend the fixed local-control request/response types and Unix and Windows transports for `CONFIGTEST`. Keep requests path-free, newline-framed, bounded, and covered by existing I/O deadlines; preserve `STOP` and `REHASH` compatibility.
4. On the daemon, perform the check against one loaded candidate and the active config snapshot on the existing serialized control/scheduler path. The check must not reconcile or publish. Return `READY` only when loading, authority, and restart-only checks pass; use `UNCHANGED` where useful to report a no-op accurately.
5. Implement the offline fallback exactly as specified: local shared-loader success may be reported as load-valid, with a prominent statement that live authority/restart compatibility was not checked. Never silently equate that with daemon acceptance.

### Phase 3: docs, tests, and qualification

1. Update CLI help and `README.md`, `crates/greggd/README.md`, `architecture/greggd-daemon.md`, `.opencode/skills/greggd-daemon/SKILL.md`, and `CHANGELOG.md`. Explain post-write persistence versus live status, the not-running behavior, the configtest offline/live distinction, and the unchanged restart-only boundary.
2. Record Plan 198 in this index and preserve Plan 196/197 acceptance records. Plan 198 supersedes only Plan 197's no-automatic-rehash exclusion for successful `greggd job add/remove`; it does not alter the rehash protocol's all-or-nothing jobs-only rules or Plan 197's outstanding qualification blockers.
3. Run focused `greggd` mutation, configtest, scheduler-policy, Unix control, and Windows control tests; run `cargo fmt --all -- --check`, focused Clippy and the repository's local check. Native Windows CI is authoritative for the Windows transport/ACL path; GNU cross-checks are development evidence only.
4. Record every failed, skipped, or unrun gate truthfully. Re-measure stripped Darwin arm64 `greggd` against the unchanged Plan 162 ceiling. Do not call the plan acceptance-green if the ceiling or applicable native qualification remains unmet, and do not relabel prior suite failures as baseline-only without evidence.

## Acceptance criteria

- [x] Successful actual `greggd job add/remove` writes the config first, releases the mutation lock, then makes exactly one config-specific local rehash attempt.
- [x] Failed, duplicate, missing, no-jobs, list, and cancelled paths do not send rehash requests; `gregg add/remove` behavior is unchanged and never targets `greggd`.
- [x] Post-write output and exit status distinguish saved-on-disk from live-applied, not-running/deferred, explicit rejection, restart-required, busy, and uncertain outcomes; no rollback or automatic retry is introduced.
- [x] `greggd configtest` honors `--config`, uses the exact rehash load cap and validation, and does not write config or claim that a check-only result was applied.
- [x] With a running daemon, configtest repeats daemon-principal authority and restart-only policy checks and reports eligibility without changing active config, scheduler jobs, publication state, history, or an active child.
- [x] Without a daemon, configtest may report local load validity only and explicitly identifies live eligibility as unchecked; it never claims `READY` in that case.
- [ ] Unix and Windows control framing stays bounded and path-free; Unix STOP and socket security remain intact, and Windows pipe ACL/remote-client rejection remain intact and receive native qualification. (The ACL half and both transports are qualified natively at `62d78f5`; remote-client rejection still needs a second Windows host.)
- [x] User docs and CLI help describe the new behavior, no-daemon and uncertain outcomes, and configtest's exact limits. Plan 196/197 and Plan 162 blockers are unchanged unless independently resolved and evidenced.

## Risks and stop conditions

- A post-write rehash failure is a partial outcome: the config may be durable while the running daemon remains on last-known-good state. Every message must distinguish those facts.
- A response timeout after request delivery must remain uncertain. Do not auto-retry, because the first request may already have applied.
- A local configtest cannot know service identity or the daemon's active restart-only baseline. Use the daemon preflight when available; label offline results as local-load-only.
- The config can change between test and later rehash. The test applies to the snapshot it loaded and is not a reservation or guarantee.
- Plan 196's possible post-rename writer error means a returned write error is not proof of no disk change; this plan must not guess or hide that separate edge.
- Plan 197 currently exceeds Plan 162's binary ceiling and has unrun native Windows qualification. Plan 198 cannot waive either blocker or claim Plan 197 closure.

## References

- Job/config mutation and CLI dispatch: `crates/greggd/src/cli.rs`.
- Complete parse/validation, bounded rehash loader, and restart-only diff: `crates/greggd/src/config.rs`.
- Local transport result types and Unix/Windows framing: `crates/greggd/src/local_control.rs`, `crates/greggd/src/control.rs`, `crates/greggd/src/control_windows.rs`, and `crates/greggd/src/control/windows_pipe.rs`.
- Serialized rehash policy, load gate, active config, and scheduler state: `crates/greggd/src/scheduler.rs`.
- Existing explicit command routing: `crates/greggd/src/main.rs`.

## Implementation and qualification record (2026-11-24)

The Plan 198 behavior is implemented in the current worktree. Successful actual `greggd job add/remove` writes make one config-specific local rehash attempt after the config mutation returns; `configtest` uses the rehash loader and the matching daemon's serialized check-only policy path. The Unix transport diagnostic was corrected so configtest disconnect/timeout reports name `configtest`, not `rehash`. The CLI docs, crate README, architecture deep dive, daemon skill, and changelog describe saved-versus-live status, offline local-only configtest, and the unchanged restart boundary. No Plan 196/197 or Plan 162 blocker is waived.

Checks on the current worktree:

- `cargo fmt --all -- --check`: passed after the final diagnostic/test edit.
- `cargo test -p greggd --all-targets --all-features cli::rehash_cli_tests`: 13 passed, 0 failed.
- `cargo test -p greggd --all-targets --all-features configtest`: final focused run passed, 7 passed and 0 failed, including the check-only diagnostic regression, offline behavior, loader bound, and scheduler non-mutation cases.
- `cargo run -q -p greggd -- configtest --help`: passed; displays `--config <PATH>` and no-apply description.
- `cargo check -p greggd --all-targets --all-features --target x86_64-pc-windows-gnu`: passed as a development cross-check.
- `cargo clippy -p greggd --all-targets --all-features --target x86_64-pc-windows-gnu --no-deps -- -D warnings`: passed as a development cross-check only. Neither replaces native Windows CI, named-pipe ACL validation, or SCM qualification.
- `cargo clippy -p greggd --all-targets --all-features -- -D warnings`: failed with 16 warnings-as-errors in `gregg-host/src/macos/ffi.rs`; these diagnostics are outside the Plan 198 changes.
- The attempted `./scripts/check-local.sh` invocation was interrupted after starting the workspace test phase; no complete exit result was captured, so it is not counted as passing.
- Final `cargo test -p greggd --all-targets --all-features` run: 567 passed, 10 failed. Eight Unix control-socket setup/identity tests failed with missing path assertions (`None`/`unwrap`); two uninstall purge tests were denied removing `/var/log/greggd.log`. These failures remain unresolved and are not classified as baseline-only. This run includes the final diagnostic change and regression test.
- After the final source/test change, `cargo build --release --target aarch64-apple-darwin -p greggd` succeeded and the Darwin arm64 release binary measured 3,555,712 bytes, 155,712 bytes over Plan 162's unchanged 3,400,000-byte ceiling.

Acceptance remains blocked: the full package run has unresolved failures, the repository local-check result is incomplete, strict Clippy fails in the macOS FFI dependency, native Windows qualification is outstanding, and the binary exceeds the frozen footprint ceiling. Plan 196 and Plan 197 statuses and their acceptance evidence remain separate and unchanged.

- Plans 196 and 197; Plan 162's unchanged footprint ceiling.

## Plan 199 follow-up qualification (2026-11-24)

Plan 199 resolves the previously failing Unix control-socket fixtures and purge tests, the post-rename writer outcome edge, and the macOS FFI strict-Clippy findings. Final local `greggd` and workspace tests, formatting, strict workspace Clippy, and complete `check-local` pass. Acceptance remains blocked by native Windows qualification and the fresh 3,555,712-byte Darwin arm64 artifact (155,712 over Plan 162's fixed ceiling). This update does not close Plan 198 or alter the independent status of Plans 196, 197, or 162.

## Subsequent footprint-limit amendment (2026-11-25)

Plan 162's original 3,400,000-byte limit remains historical; by explicit user
approval, the current prospective ceiling is 3,700,000 bytes. The restored-
default 3,555,696-byte Darwin arm64 artifact is within the amended limit by
144,304 bytes. This changes only the applicable size criterion; native Windows
qualification and the distinct Plan 196/197/198 acceptance statuses remain as
recorded.

## Native Windows ACL qualification and the remaining item (2026-11-25)

- The ACL half of the open criterion is now evidenced on native Windows rather
  than by inspecting the SDDL constant:
  `created_pipe_object_carries_the_protected_dacl` opens the created pipe and
  reads the security descriptor actually attached to the object (a protected
  DACL whose only ACEs grant Administrators and LocalService), and
  `a_deny_only_administrators_token_cannot_connect` impersonates a restricted
  token whose Administrators group is deny-only and asserts the connect fails
  with `ERROR_ACCESS_DENIED`.
- Both transports are qualified natively: the `control_windows.rs`
  REHASH/CONFIGTEST exchange tests cover the bounded, path-free request and its
  acknowledgment over the pipe, and the Unix control suite covers `STOP`,
  socket identity, permissions, and the same config requests.
- Hosted CI run `45725598320` at `62d78f5` passed the native Windows job's
  Clippy, full-workspace tests, both release builds, and the SCM lifecycle
  smoke.
- `PIPE_REJECT_REMOTE_CLIENTS` is still not evidenced. A local client, however
  it is built, cannot demonstrate that a *remote* client is refused, and Plan
  199 forbids substituting a cross-check for that evidence. This remains the
  only open criterion in Plan 198.
