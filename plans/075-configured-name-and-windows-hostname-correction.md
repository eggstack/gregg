# Phase 075: configured name and Windows hostname correction

Status: complete.

Depends on: Plans 073-074.

## Objective

Correct two small identity defects exposed by the native Windows CI smoke without reopening the completed SCM architecture or expanding Gregg's scope:

1. Windows native hostname collection can retain a trailing NUL from `GetComputerNameExW`, which is then serialized as `\u0000` in v2 identity fields.
2. `Config::name` is documented as the human-readable daemon name, but foreground startup and the Windows SCM worker construct collectors with `None`, so the configured name is ignored and `system.name` falls back to the native hostname.

This phase is a narrow product-correctness pass. Do not add a new protocol field, configuration option, identity framework, service abstraction, workflow, CI job, or evidence system.

## Scope

### In scope

- Fix Windows hostname buffer-length handling in `crates/greggd/src/collector/windows/source.rs`.
- Pass the validated configured daemon name into native collector construction in foreground mode.
- Pass the same configured name into Windows SCM collector construction.
- Strengthen the existing Windows foreground and SCM smoke coverage to prove both corrections.
- Run the existing local checks and one ordinary CI run.
- Mark this plan complete and return the plan index to no active corrective work after the green run.

### Out of scope

- Changes to v1/v2 schemas or protocol compatibility.
- Hostname rewriting, DNS canonicalization, FQDN discovery, NetBIOS aliases, or new identity sources.
- Changes to configuration validation or the meaning of `Config::name`.
- Collector redesign or a generic collector factory solely to pass one string.
- Any SCM lifecycle, installer, scheduler, TUI, drive, EggPool, release, or dependency work.
- New workflows, jobs, matrices, artifacts, self-hosted runners, evidence bundles, or Plan 076 created only for closure.

## Authoritative behavior

After this phase:

- `system.hostname` remains the native platform hostname.
- `system.name` equals the validated configured `Config::name` when a daemon is launched from configuration.
- Windows `system.hostname` and `system.name` contain no trailing or embedded NUL produced by the native hostname API.
- The behavior is the same for foreground daemon startup and Windows SCM startup.
- Existing collector APIs remain unchanged: native collectors already accept `Option<&str>` display-name overrides.

## Implementation

### 1. Correct `GetComputerNameExW` result handling

File:

```text
crates/greggd/src/collector/windows/source.rs
```

In `get_hostname()`:

- keep the existing two-call `GetComputerNameExW` pattern;
- after the successful second call, treat the returned `size` as the number of UTF-16 code units written for the hostname;
- truncate the allocated buffer to that returned length before `String::from_utf16`;
- remove the current assumption that popping one final allocated zero is sufficient;
- preserve the existing errors for zero required size, native call failure, and invalid UTF-16.

Do not introduce general UTF-16 normalization machinery. The fix should remain local to this Windows API boundary.

Add focused native-Windows regression coverage in the existing `source.rs` Windows test module or the existing Windows integration smoke proving the production hostname contains no `\0` character.

### 2. Honor `Config::name` in foreground startup

File:

```text
crates/greggd/src/main.rs
```

In the `Command::Run` branch:

- load and validate config exactly as today;
- construct `NativeCollector` with `Some(config.name.as_str())` instead of `None`;
- then pass the same config unchanged into the existing daemon runtime.

Do not add a collector factory, trait object, cloned configuration object, or separate display-name state.

Because this code is compiled natively by the existing Linux, macOS, and Windows CI jobs, the same direct wiring must remain valid on all supported daemon platforms.

### 3. Honor `Config::name` in Windows SCM startup

File:

```text
crates/greggd/src/service/windows.rs
```

In `run_service_worker()`:

- load the selected config path exactly as today;
- construct `WindowsCollector` with `Some(config.name.as_str())` instead of `None`;
- preserve the existing single current-thread runtime, dispatcher/`ServiceMain` boundary, post-bind `RUNNING` publication, shutdown channel, and status handling.

Do not modify SCM launch-context handling or service lifecycle semantics.

### 4. Strengthen existing Windows foreground smoke

File:

```text
crates/greggd/tests/windows_smoke.rs
```

The existing `foreground_daemon_serves_v2_status` test already writes:

```toml
name = "smoke-test"
```

Extend only its identity assertions so it proves:

- `system.name == "smoke-test"`;
- `system.hostname` is nonempty;
- `system.hostname` contains no NUL;
- `system.name` contains no NUL.

Keep the existing status/capability/metric assertions. Do not create another Windows integration binary or test harness.

### 5. Strengthen the existing Windows SCM smoke

File:

```text
scripts/smoke-windows.ps1
```

After the existing successful health/status fetch, parse or inspect the response with the smallest existing PowerShell mechanism and assert:

- returned `system.name` is exactly the configured `smoke-test` value;
- returned `system.hostname` is nonempty;
- neither identity value contains `[char]0`.

Retain the existing start/stop/restart, custom config path, bind-failure, recovery, reinstall, and cleanup coverage unchanged.

Do not add another service cycle solely for these assertions.

### 6. Verification

Run focused checks first:

```bash
cargo fmt --all -- --check
cargo test -p greggd
./scripts/check-local.sh
```

The existing native Windows CI job remains the authoritative Windows environment. Require one ordinary workflow run at the implementation SHA or a source-equivalent documentation-only descendant.

The Windows job must remain green through its existing sequence:

```text
workspace tests
release greggd build
Windows SCM lifecycle smoke
```

No additional workflow, job, artifact, retry layer, or qualification pass is required.

### 7. Record closure directly

After one green ordinary CI run:

- mark Plan 075 complete;
- update `plans/README.md` to show no active corrective phase;
- record only the implementation SHA and workflow run ID in this plan or the index;
- leave Plans 066-074 complete and historically accurate;
- do not create Plan 076 or an evidence file.

Closure record:

- Implementation SHA: `069bf37`
- Workflow run: `31189587467` (all Linux, Windows, macOS, and MSRV jobs green)

## Acceptance criteria

### Windows hostname correctness

- [x] `GetComputerNameExW` output is truncated using the successful call's returned length.
- [x] Native Windows hostname serialization contains no trailing `\u0000`.
- [x] Existing invalid/empty hostname error behavior is preserved.
- [x] No new Windows identity dependency or abstraction is introduced.

### Configured display name

- [x] Foreground daemon startup passes `Config::name` into the native collector.
- [x] Windows SCM startup passes the selected config's `name` into `WindowsCollector`.
- [x] `system.name` equals the configured name while `system.hostname` remains the native hostname.
- [x] No config schema, validation, or CLI behavior changes.

### Regression coverage

- [x] Existing Windows foreground smoke asserts `system.name == "smoke-test"`.
- [x] Existing Windows foreground smoke rejects NUL-containing name/hostname values.
- [x] Existing Windows SCM smoke asserts the configured name and NUL-free hostname from the live service response.
- [x] No new test framework, workflow, or Windows job is added.

### Verification and scope

- [x] `cargo fmt --all -- --check` passes.
- [x] Focused greggd tests pass.
- [x] `./scripts/check-local.sh` passes.
- [x] One ordinary CI run is green, including the native Windows SCM lifecycle smoke.
- [x] Plans 073-074 remain closed and their SCM architecture is unchanged.
- [x] Plan 075 and `plans/README.md` describe the demonstrated final state.
- [x] No Plan 076, evidence document, protocol work, or unrelated cleanup is added.

## Handoff

Report only:

```text
Implementation SHA: `069bf37`
Hostname correction: `GetComputerNameExW` output is truncated to its successful UTF-16 length; Windows foreground and SCM smoke coverage rejects NUL identity values.
Configured-name wiring: `Config::name` is passed to native collectors in foreground and Windows SCM startup.
Regression coverage: focused `greggd` tests, full local/release preflight, and the existing Windows foreground/SCM smokes.
Workflow run: `31189587467` — green across Linux, Windows, macOS, and MSRV.
Planning-record closure: Plan 075 complete; `plans/README.md` has no active corrective phase; no Plan 076 or evidence file created.
```
