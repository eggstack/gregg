# Phase 069: daemon CLI, runtime, and test correctness

Status: complete.

Depends on: Plan 066. May run after or alongside Plans 067-068, but should rebase on their final state before closure.

## Objective

Correct first-run daemon configuration mutation, keep process termination and global logging at the binary boundary, make the existing exit-code taxonomy truthful, and restore a scheduler test that is currently compiled as unused code rather than executed.

This is a direct correctness pass, not a new CLI or error architecture.

## Owned defects

1. `greggd` resolves the config path in `main`, but `cli::dispatch()` hardcodes the path as explicitly supplied. Therefore `greggd host ...` and `greggd port ...` fail when the platform-default config is absent instead of starting from defaults.
2. `run_with_shutdown()` returns `Result` but calls `std::process::exit()` for validation failures.
3. Daemon runtime code initializes the global tracing subscriber with `.init()`, which can panic in an embedding or test process where logging is already installed.
4. The daemon defines stable exit-code categories, but `main()` returns generic `Result` errors for many paths rather than centrally applying them.
5. `scheduler_produces_batches_with_increasing_generations` lacks `#[tokio::test]` and is never run.
6. Blanket module-level `allow(dead_code)` attributes can conceal similar omissions.

## Scope

### In scope

- Preserve whether `--config` was supplied and pass that fact to config-loading mutations.
- Permit first-run `host` and `port` mutation at the default path.
- Continue rejecting an explicitly supplied missing config path.
- Return errors from library/runtime functions instead of exiting.
- Move or safely initialize logging at the binary boundary.
- Apply the existing daemon exit-code enum centrally or remove unreachable variants if they cannot be truthfully classified.
- Add the missing scheduler test attribute.
- Narrow blanket dead-code allowances in directly affected client modules.
- Add focused regression tests for all corrected behavior.

### Out of scope

- New daemon commands, interactive config creation, config migration, or environment-variable config.
- Redesigning service managers or service installation.
- Introducing `anyhow`, a new error crate, a diagnostic framework, or a generic command dispatcher.
- Changing user-facing exit-code numbers unless a current number is impossible to preserve.
- Rewriting the poll scheduler; that is conditionally assessed in Plan 070.
- Removing every warning allowance across the repository.

## Expected files

```text
crates/greggd/src/main.rs
crates/greggd/src/cli.rs
crates/greggd/src/run.rs
crates/greggd/src/config.rs          # tests/helpers only if needed
crates/gregg/src/scheduler.rs
crates/gregg/src/poller.rs           # only for narrowed allowances if compilation requires it
crates/gregg/src/eggpool.rs          # only for narrowed allowances if compilation requires it
README.md
architecture/greggd-daemon.md
```

## Implementation sequence for GPT-5.6 Luna

### Step 1: write config-intent regression tests

Cover both branches explicitly:

1. Default config path absent and `explicit == false`: `host` or `port` mutation starts from `Config::default()`, writes the resulting config, and restarts the service.
2. Custom `--config` path absent and `explicit == true`: mutation fails with a not-found config error and does not restart the service.
3. Existing default or custom config: mutation preserves unrelated fields and restarts once.
4. Validation failure: config is not written and service is not restarted.

Use a temporary path and the existing fake service manager. Do not write to platform system directories in tests.

### Step 2: propagate explicit config intent directly

At the binary boundary retain:

```rust
let config_was_explicit = cli.config.is_some();
let config_path = resolve_config_path(cli.config.as_ref());
```

Pass both values to dispatch or to the two mutation commands. Remove the hardcoded `let explicit = true`.

Prefer a small signature change:

```rust
dispatch(command, config_path, config_was_explicit, service)
```

Do not add a context object for two values.

### Step 3: remove process exits from runtime/library code

`run_with_shutdown()` and related library functions must return errors. Replace validation branches that print and exit with an existing typed error where available, or one small daemon-run error enum if the current types cannot express the condition.

Requirements:

- no `std::process::exit()` below `crates/greggd/src/main.rs`;
- no duplicate printing in both library and binary layers;
- server bind, sampler interval, configuration, service, and permission failures remain distinguishable enough for exit-code classification;
- shutdown success still exits zero.

Do not generalize all crate errors into one large hierarchy.

### Step 4: keep logging initialization at the executable boundary

Move tracing initialization out of reusable runtime functions. In `main` or a binary-only helper, use `try_init()` and handle `SetGlobalDefaultError` without panic.

Acceptable behavior:

- normal standalone execution installs the configured/default subscriber;
- tests or embedding with an existing subscriber continue without panic;
- no logging configuration file, reload support, or subscriber abstraction is added.

If Windows SCM service mode needs logging initialization, call the same binary-local helper from that branch.

### Step 5: centralize daemon exit-code application

Retain the existing numeric contract where practical:

```text
0 success
1 configuration error
2 service error
3 runtime error
4 permission denied
```

Prefer this direct structure:

```rust
fn main() {
    let code = match run_main() {
        Ok(()) => ExitCode::Success,
        Err(error) => {
            eprintln!("error: {error}");
            classify_exit_code(&error)
        }
    };
    std::process::exit(code as i32);
}
```

`run_main()` may remain async under `#[tokio::main]` and return `Result`. Classification may use the existing concrete error types and limited downcasting, as the client already does. Do not add a dependency or a generic error-reporting layer.

Required tests:

- config parse/not-found maps to config error;
- permission-denied I/O maps to permission denied;
- service manager errors map to service error or permission denied as already defined;
- bind/sampler/runtime errors map to runtime error;
- successful commands map to zero.

If a current `ExitCode` variant is truly unreachable after inspection, delete it and update documentation rather than manufacturing a path solely to exercise it. Preserve numeric values of retained variants.

### Step 6: restore the omitted scheduler test

Add `#[tokio::test]` to `scheduler_produces_batches_with_increasing_generations` and make any minimal timing corrections needed for deterministic execution.

The test must verify:

- the first batch generation is 1;
- a later trigger produces generation 2;
- cancellation closes the run cleanly.

Use paused time or the existing fake clock only if the test already supports it. Do not sleep for long wall-clock intervals.

### Step 7: narrow dead-code suppression

Inspect module-level `#![allow(dead_code)]` in the directly affected client modules.

Rules:

1. Remove a blanket allowance when production and test items compile without it.
2. If an item is intentionally test-only, gate it with `#[cfg(test)]` or apply an item-level allowance with a short reason.
3. Delete genuinely unused private helpers.
4. Do not convert private items to `pub` merely to silence warnings.
5. Do not expand this into a repository-wide warning cleanup.

The scheduler module should no longer hide a missing test attribute behind a blanket allowance.

### Step 8: reconcile command documentation

Document that default-path `host`/`port` commands can initialize configuration, while `--config PATH` requires that path to exist. Keep the text short and match implemented behavior.

## Focused verification

```bash
cargo test -p greggd cli
cargo test -p greggd run
cargo test -p greggd --bin greggd
cargo test -p gregg scheduler
cargo clippy -p greggd --all-targets --all-features -- -D warnings
cargo clippy -p gregg --all-targets --all-features -- -D warnings
./scripts/check-local.sh
```

Do not add a new integration harness or CI command. Existing ordinary CI provides native Windows service compilation truth.

## Acceptance criteria

- [ ] Default-path `host` and `port` mutation can initialize an absent config from defaults.
- [ ] Explicit missing `--config` paths still fail without writing or restarting.
- [ ] `cli::dispatch` no longer hardcodes explicit-path behavior.
- [ ] No library/runtime function calls `std::process::exit()`.
- [ ] Logging initialization cannot panic when a subscriber is already installed.
- [ ] The daemon binary applies retained exit-code categories centrally and prints each failure once.
- [ ] Exit-code tests cover config, permission, service, runtime, and success paths.
- [ ] `scheduler_produces_batches_with_increasing_generations` is an executed Tokio test.
- [ ] Blanket dead-code suppression is removed or narrowed in directly affected modules.
- [ ] No new CLI command, error dependency, logging framework, or scheduler rewrite is introduced.
- [ ] Focused tests, focused Clippy, and the default local check pass.

## Handoff format

Report the corrected config-intent flow, exit-code mapping, logging boundary, restored test name, narrowed allowances, and verification results. Do not create an evidence file.

## Completion

Config intent, runtime error returns, binary exit classification, non-panicking
logging initialization, and the executed scheduler generation test are fixed.
