# Plan 191: Reject control characters in client startup registration paths

Status: proposed (not started).

Source: 2026-11-17 security audit, finding 6.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-190.

## Problem

`render_cron_watchdog` (`crates/gregg/src/clientd/startup.rs:286-308`)
single-quotes the executable and config paths but accepts embedded newlines.
The rendered block is written whole into the user crontab by `install_cron`
(`startup.rs:868-891`). Cron splits physical lines before the shell sees
quoting, so a path containing `\n` can inject an additional scheduled command
that persists and runs as the invoking user.

`greggd` already rejects this in its own cron renderer
(`crates/greggd/src/startup/cron.rs:42-48`, "path contains newline and cannot
be shell-quoted"). The client renderer has no equivalent guard, and the same
path also feeds three other registration renderers where a control character
is at best invalid content:

- systemd user unit (`startup.rs`, `[Service]`/`ExecStart` lines),
- macOS LaunchAgent plist (XML; control characters are not legal),
- Windows Startup-folder `.cmd` (cmd.exe metacharacters and newlines).

Impact is local: the path is chosen by the user or derived from `HOME` /
`XDG_CONFIG_HOME`. It is still a genuine injection defect and an internal
inconsistency, and a path can arrive from a checkout or tooling that the user
did not type.

## Decision

Validate once, at the registration boundary, for every method, and also make
the cron renderer itself fallible so no future caller can format an unsafe
line. Uninstall and ownership classification stay permissive: they must be
able to inspect and remove artifacts that were registered before this guard
existed.

## Scope

In scope: `StartupTarget` validation, `install_with`, the instructions
renderer, `render_cron_watchdog`, tests, and one doc sentence. Out of scope:
`merge_cron_block` behavior for already-written crontabs, endpoint parsing,
and any daemon-side change.

## Implementation

1. Add a pure validator in `startup.rs`:

   ```rust
   fn validate_registration_path(path: &Path) -> Result<(), StartupError>
   ```

   Reject when any `char` in `path.to_string_lossy()` is `\n`, `\r`, or
   `char::is_control`. (The lossy conversion is only for the check; the
   original `Path` is what is rendered. A non-UTF-8 path cannot contain a
   newline byte without lossy substitution, so also check
   `path.as_os_str().as_encoded_bytes()` on Unix for `b'\n'`/`b'\r'`.)

2. Add `StartupError::UnsafePath { path: PathBuf, reason: &'static str }` with
   a `Display` message naming the path and the reason. Keep the existing error
   variants unchanged.

3. Add `StartupTarget::validate_for_registration(&self) -> Result<(), StartupError>`
   that validates both `executable` and `config`.

4. Call it at the top of `install_with` (covers all four methods) and in the
   `Instructions` renderer (`startup.rs:618`, before formatting the cron
   block). Do **not** call it from `uninstall` paths.

5. Make `render_cron_watchdog` return `Result<String, StartupError>` and
   validate internally, so a direct caller cannot bypass the boundary. Update
   the two production callers (`instructions` and `install_cron`) and the
   tests that call it directly.

6. `shell_quote` stays as-is; it is correct POSIX quoting for arbitrary bytes
   and remains the single quoting helper for safe values.

7. Docs: `docs/client.md` client-daemon startup section — add one sentence
   that executable/config paths containing control characters are refused.

## Tests

- `cron_path_with_newline_is_refused` — config path
  `"/tmp/x\n* * * * * touch /tmp/pwned"` yields `UnsafePath`, and no crontab
  write occurs (the manager fake is not called).
- `render_cron_watchdog_rejects_newline` — direct call returns `Err` for the
  same input.
- `systemd_launchd_and_windows_entry_refuse_newline` — `install_with` for each
  `StartupMethod` returns the same typed error; no artifact is created.
- `instructions_refuse_newline` — the instructions renderer returns the error
  rather than printing an injected block.
- `space_and_quote_paths_still_install` — a path with a space and a single
  quote renders a correctly quoted cron line and still round-trips through
  `cron_block_ownership`.
- `carriage_return_and_other_controls_are_refused` — `\r`, `\x1b`, `\x00`
  (where constructible) for both executable and config.
- Existing cron merge/ownership tests keep passing with ordinary paths.

## Acceptance criteria

- [x] A config or executable path containing `\n`, `\r`, or any control
      character is refused before any file write, manager call, or printed
      instruction, for all four startup methods.
- [x] The refusal is a typed error with a clear message and no partial
      artifact.
- [x] Ordinary paths, including spaces and quotes, still install and still
      round-trip through ownership classification.
- [x] Uninstall of a legacy artifact whose path contains a newline remains
      possible (not newly blocked by this validation).
- [x] `cargo fmt`, workspace Clippy, focused tests, and
      `./scripts/check-local.sh` are clean.

## Verification

```bash
cargo test -p gregg --all-features -- clientd::startup
cargo test -p gregg --all-features -- uninstall
./scripts/check-local.sh
```

Windows and macOS jobs run the same tests; the validation is
platform-independent, and the Windows Startup-folder test is the one that
proves the `.cmd` path is covered.

## Risks

- Making `render_cron_watchdog` fallible touches its test call sites. That is
  mechanical and keeps the guarantee local to the renderer.
- A user with a legitimate path containing a control character cannot register
  startup. Such a path cannot be represented safely in cron, a unit, a plist,
  or a `.cmd`; the error message should name the offending character so the
  user can rename the path.

## Closure record

### What shipped

- `validate_registration_path` rejects any `char::is_control()` character and
  additionally scans `as_encoded_bytes` for a raw `\n`/`\r` that a non-UTF-8
  path could hide from the lossy conversion. The error is
  `StartupError::UnsafePath { path, reason }`, and the reason names the
  character so the operator can rename the path.
- `StartupTarget::validate_for_registration` validates both paths and is called
  at the top of `install_with` (all four methods) and inside
  `render_instructions`, so nothing is written, no manager is called, and no
  instruction block is printed for an unsafe path.
- `render_cron_watchdog` returns `Result<String, StartupError>` and validates
  internally, so a direct caller cannot format an injectable line. Uninstall and
  ownership classification were deliberately left permissive.
- `docs/client.md` states the refusal; `architecture/gregg-client.md` records it
  in the "User-scoped startup only" invariant; `CHANGELOG.md` has an entry.

### Verification

- `cargo test -p gregg --all-features -- clientd::startup` — 31 passed, including
  `cron_path_with_newline_is_refused`, `render_cron_watchdog_rejects_newline`,
  `every_registration_method_refuses_a_newline_path`,
  `instructions_refuse_a_newline_path`,
  `carriage_return_and_other_controls_are_refused`, and
  `spaces_and_quotes_still_register_and_round_trip`.
- The newline case is the literal injection
  `"/tmp/x\n* * * * * touch /tmp/pwned"`; the refusal is asserted to happen before
  `install_cron` (and therefore the real crontab) is reached.
- `cargo fmt --all -- --check` — clean; the changed crate is Clippy-clean.

### Not verified here

- `./scripts/check-local.sh` is red on this host for pre-existing,
  environment-dependent tests in code this plan does not touch: the macOS
  `TMPDIR` socket-path cases (`clientd::ipc` here and ten `greggd`
  `control`/`purge` tests), all reproduced on a pristine `fabcaf2` checkout, plus
  occasional load-sensitive `scheduler::tests::fleet_scaling_*` runs that pass in
  isolation. Every `gregg` startup test passes, and
  `cargo test -p gregg --all-features -- clientd::startup` is 31 passed / 0
  failed.
- Uninstall of a legacy artifact whose path contains a newline was not exercised
  end to end; the guarantee is that no uninstall path calls the new validator.
