# Plan 196: greggd scheduled-job CLI and config editor

Status: implementation present; acceptance blocked by the measured binary-footprint limit, a post-rename atomic-writer error edge, and incomplete verification.

Source: user request for a simple CLI to add, list, and remove locally scheduled `greggd` jobs, plus a full daemon-config editor matching `gregg edit` behavior.

Depends on: the existing scheduled-job schema and scheduler from Plans 157-160, and current post-195 main.

## Objective

Add a small, local-only management interface for the scheduled jobs already supported by `greggd`. Operators should be able to add a validated job, inspect configured jobs, remove one by name or choose one interactively, and edit the full daemon configuration with the same transactional editor behavior as `gregg edit`. All commands use the existing `--config` path selection and `Config` validation/persistence. The daemon continues to read configuration at startup; CLI mutations do not restart or signal a running daemon.

## User-facing contract

Use `job`, not `cron`, because `greggd startup ... --method cron` already names the OS startup integration.

```text
greggd job add <NAME> --schedule '<5-field schedule>' [JOB OPTIONS] -- <PROGRAM> [ARGS...]
greggd job list
greggd job remove [NAME]
greggd edit
```

Examples:

```text
greggd job add rotate-logs --schedule '0 2 * * *' -- /usr/local/bin/rotate-logs --keep 14
greggd job list
greggd job remove rotate-logs
greggd job remove
greggd edit
```

- `job add` requires a name, the existing five-field local-time schedule, and a non-empty executable/argv after `--`. Optional job flags map only to existing `ScheduledJobConfig` fields: `--working-dir`, `--max-load`, `--load-window` (`1m`, `5m`, or `15m`), `--retry-interval-ms`, and `--max-wait-ms`. Omitted optional values retain the scheduler's current defaults. The existing config validator remains authoritative for names, schedules, bounds, and option combinations; duplicate names and invalid jobs fail without writing.
- Store the executable and each argument as separate argv entries. The CLI does not interpolate, concatenate, or invoke a shell; the existing scheduler spawn path is unchanged. The `--` separator makes the command boundary explicit.
- `job list` is read-only and lists the configured file in config order, including the schedule, command/arguments, and configured working-directory/load-gate details. Render argv safely as individual escaped values, not as a purported shell-ready command.
- `job remove NAME` removes that exact configured name without prompting and is suitable for scripts. A missing name opens a bounded interactive selector: Up/Down or `j`/`k` moves, Enter removes, and Escape/`q`/Ctrl-C cancels. Cancellation and non-TTY refusal do not write the config. With no jobs, report that fact and exit successfully. There is no remove-all behavior.
- `greggd edit` opens the complete daemon config selected by `--config` or the normal default path; it is not a separate job-only editor. Match `gregg edit`'s editor selection and transaction semantics, but implement within `greggd` and its existing config layer. Launch the editor directly with argv, never through a shell.
- Mutating commands edit a candidate config, validate it, and commit through the existing atomic config writer. An editor failure, invalid TOML/config, duplicate job, unknown removal name, or cancelled selector leaves the original config untouched. `job list` and cancellation are read-only.
- Commands only change the config on disk. They do not contact a running daemon, alter the scheduler API, or restart/reload the service. The updated jobs take effect on the next daemon start; help and success output must state this clearly.
- Preserve the explicit root-job opt-in. `job add` must never turn on `allow_privileged_jobs` implicitly; the existing authority check and default remain unchanged.

## Scope

In scope:

- Add a nested `greggd job` CLI command with `add`, `list`, and `remove`, plus top-level `greggd edit`.
- Reuse the current config path resolution, job schema, validation, atomic persistence, scheduler defaults, and direct-argv execution contract. No configuration schema migration is expected.
- Keep named removal deterministic and noninteractive; use an interactive selector only when the name is omitted, with terminal detection and guaranteed terminal-state restoration on every exit path.
- Implement editor transaction behavior equivalent to `gregg edit` using `greggd`-owned code; do not make `greggd` depend on the `gregg` crate or invoke `gregg` as a subprocess.
- Update `README.md`, `crates/greggd/README.md`, `architecture/greggd-daemon.md`, `.opencode/skills/greggd-daemon/SKILL.md`, `CHANGELOG.md`, and this index for the user-visible behavior.

Preserved exclusions:

- No `cron` CLI namespace, `job edit`, run-now, enable/disable, job history, or scheduler-status commands.
- No HTTP/API or remote scheduler mutation, client-daemon integration, persistent job database, config schema change, or scheduler execution/cadence redesign.
- No shell wrapper, implicit shell parsing, service-manager change, implicit daemon restart/reload, or `gregg` crate dependency.
- No implicit change to `allow_privileged_jobs`; no weakening of the daemon's same-principal execution or existing service sandboxing.
- No new CI workflow, job, matrix, or broad TUI framework. Use `crossterm` only for raw key input and simple selector redraw if needed; do not add Ratatui. Measure the stripped daemon size and preserve the existing Plan-162 3,400,000-byte ceiling; if the minimal selector crosses it, stop and report the measured trade-off instead of silently raising the budget or changing the agreed selector behavior.
- No unrelated config/editor refactor or broad CLI cleanup.

## Phase 1: CLI and job config operations

1. Add the nested Clap command model and help text for `job add/list/remove`; preserve global `--config` behavior and make the trailing executable/arguments unambiguous with `--`.
2. Implement `job add` through the existing config load, validation, and atomic-write path. Append one `ScheduledJobConfig` without changing unrelated config fields. Reject invalid/duplicate input before commit and preserve argv boundaries exactly.
3. Implement `job list` as a read-only rendering of the selected config. Keep the output truthful about configured values and never format a shell command that suggests different execution semantics.
4. Implement named removal as an exact, noninteractive config mutation. For an omitted name, first verify stdin/stdout are terminals, then show the selector with the specified keys; restore terminal mode on success, cancel, input error, and early return. Cancellation must not call the writer.
5. Keep the selector small and owned by `greggd`. If direct terminal input is required, add only the minimal `crossterm` dependency and use no Ratatui; measure the stripped binary against the existing ceiling before accepting the dependency.

## Phase 2: full daemon config editor

1. Add top-level `greggd edit` using the same editor discovery/launch behavior as `gregg edit`, scoped to the daemon config path.
2. Edit a candidate copy rather than exposing the live config to partial writes. Launch the editor directly (no shell), require successful editor exit, parse and validate the entire candidate, and only then commit using `Config::write_atomic`.
3. Preserve the original file on editor launch/exit failure, invalid TOML, validation error, or failed atomic commit. Report actionable validation diagnostics. The editor may change any existing daemon setting, not only jobs.
4. Do not restart a running daemon after `edit` or a job mutation. Printed success/help text must identify the config path and say changes apply on the next daemon start.

## Phase 3: regression coverage, docs, and verification

1. Add deterministic Clap/CLI tests for all command forms, global config selection, missing/extra arguments, the `--` argv boundary, and stable help behavior.
2. Add config mutation tests proving add preserves unrelated fields and argv boundaries, rejects duplicate/invalid jobs without changing file bytes, list is read-only, named removal removes only the exact job, and the root opt-in remains unchanged.
3. Test selector movement, empty-list behavior, named removal without a TTY, no-name non-TTY refusal, cancellation without writes, and terminal restoration on every error/exit path. Include a PTY smoke where the existing test tooling supports one.
4. Test editor success and failure with a controlled editor executable: valid edits commit atomically; nonzero exit, invalid TOML/config, and launch failure leave the original file intact. Verify the editor is launched without a shell.
5. Update the user documentation named in Scope with examples for add/list/named and interactive remove/edit, the direct-argv rule, config-path behavior, restart boundary, and unchanged root-job opt-in. Record the feature in `CHANGELOG.md` and reconcile this plan's index entry.
6. Run focused `greggd` tests, `cargo fmt --all -- --check`, `cargo clippy -p greggd --all-targets --all-features -- -D warnings`, and `./scripts/check-local.sh`. Run the existing native CI workflow; the native Windows job is required for CLI/terminal platform truth. Do not add CI infrastructure.

## Acceptance criteria

- [x] The documented command forms are implemented: `greggd job add`, `greggd job list`, `greggd job remove [NAME]`, and top-level `greggd edit`.
- [x] `job add` validates against the existing schema, atomically stores the exact executable/argv and optional settings, and never invokes the job or a shell.
- [x] `job list` reads only the selected config and presents its configured jobs without mutation or shell-command ambiguity.
- [x] Named removal is noninteractive and exact; omitted-name removal offers the specified selector, and cancel/non-TTY/error paths leave the config unchanged with the terminal restored. (PTY behavior was not exercised.)
- [ ] `greggd edit` edits the whole selected daemon config transactionally; invalid or failed edits cannot replace the last valid config.
- [x] Every mutation uses existing validation and atomic persistence; `--config` is honored; no daemon restart/reload is implicit and changes take effect on next daemon start.
- [x] Existing root-job opt-in and direct-argv scheduler execution remain unchanged; no `gregg` dependency, remote mutation surface, schema migration, or scheduler behavior change is introduced.
- [x] The selector adds no Ratatui/UI framework, and stripped `greggd` remains within the user-approved 3,700,000-byte amended Plan-162 ceiling (matched Plan 196 artifact: 3,472,944 bytes).
- [ ] Focused tests, local checks, docs inspection, and the existing native CI workflow pass; no new workflow/job/matrix is added.

## Risks and stop conditions

- A raw-key selector has a real terminal/dependency cost. Keep it to a small `greggd`-owned selector and the minimum terminal-input support; if that exceeds the documented binary ceiling, do not silently raise the budget or substitute a different interaction model.
- The config editor must not edit the live file in place. Any failure before a validated atomic commit must preserve the prior bytes.
- A scheduled command must remain a vector of argv values. Any implementation that joins arguments and later reparses them through a shell violates the existing execution boundary and stops this plan.
- Do not make `job add` appear to enable root execution. The explicit `allow_privileged_jobs` setting remains operator-controlled and defaults false.
- The daemon does not watch for CLI config changes; documentation must not imply that an already-running instance updates immediately.

## Implementation and qualification record (2026-11-24)

The requested command and editor implementation is present; six behavioral acceptance criteria are checked above. Criterion 5 remains open because the existing write_atomic routine has fallible operations after rename: a set-permissions or non-permission parent-directory sync error can be returned after the live file has already been replaced. Pre-commit editor, parse, and validation failures preserve the original bytes. This post-rename error edge was not changed while the explicit Plan-162 footprint stop condition is in force.

Acceptance is also blocked by that footprint condition: the stripped Darwin arm64 release binary measured 3,472,944 bytes, 72,944 above the 3,400,000-byte ceiling. The selector and budget were not changed to bypass the gate.

Focused checks passed: cargo fmt --all; cargo clippy -p greggd --all-targets --all-features --no-deps -- -D warnings; and 10 targeted CLI, selector-helper, config-transaction, editor-invocation, and error-classification tests. The full cargo test -p greggd --all-targets --all-features run reported 538 passed and 10 failed: eight untouched control::tests failed with Option::unwrap() on None, and two untouched uninstall tests were denied while removing /var/log/greggd.log. The full strict Clippy command also remains blocked by 16 diagnostics in untouched gregg-host/src/macos/ffi.rs. The default ./scripts/check-local.sh ran and failed in the gregg suite: 988 passed, 2 failed, and 2 were ignored. The failures were eggpool::tests::worker_cancellation_aborts_an_in_flight_request and eggpool::tests::worker_result_timestamps_come_from_the_injected_clock; both are outside the Plan 196 changes. No PTY smoke or native Windows CI was run; the final verification criterion remains unchecked.

## References

- Existing scheduled-job schema, validation, and atomic writer: `crates/greggd/src/config.rs`.
- Existing CLI and config mutation boundary: `crates/greggd/src/cli.rs`.
- Existing scheduler behavior and startup config loading: `crates/greggd/src/run.rs` and Plans 157-160.
- Reference editor behavior and selector interaction: `crates/gregg/src/config/store.rs`, `crates/gregg/src/cli.rs`, and `crates/gregg/src/cli/selector.rs`.
- Workspace layering and user-visible documentation requirements: `AGENTS.md` and `architecture/workspace.md`.

## Plan 199 corrective qualification (2026-11-24)

Plan 199 corrected the writer contract: final permissions are prepared before rename, and a parent-directory sync failure after rename returns `CommittedButDurabilityUncertain` rather than implying rollback. Deterministic pre-rename preservation and post-rename committed-state regressions pass. The final local `greggd`, workspace, formatting, strict-Clippy, and `check-local` gates pass; see Plan 199 for exact totals.

Plan 196 remains acceptance-blocked. A matched source build of commit `2df76255c9f49a3d465294dd9fd90b7a706dac55` under the current release profile measured 3,472,944 bytes, 72,944 above the then-current 3,400,000-byte ceiling. Native Windows qualification is also outstanding. This follow-up updates the writer and local-gate evidence only; it does not close Plan 196 or alter Plan 162.

## Subsequent footprint-limit amendment (2026-11-25)

Plan 162's original 3,400,000-byte limit remains historical; by explicit user
approval, the current prospective ceiling is 3,700,000 bytes. The matched Plan
196 artifact at 3,472,944 bytes is within that limit by 227,056 bytes. This
updates the footprint criterion only. Plan 196 remains open on its other
acceptance criteria, including native Windows/PTY qualification; no feature or
behavior change was made to obtain this result.