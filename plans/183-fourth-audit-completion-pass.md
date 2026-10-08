# Plan 183: Fourth-audit completion pass (ownership, packaging, and the unreviewed leaf crates)

Status: complete. See the closure record at the end.

Depends on: current main at `aa321dd` ("gregg, greggd: fix eight audited defects", Plan 182).

Opened from the coverage gap Plan 182 disclosed in its own closure record: the
audit's fourth pass never completed, leaving `crates/gregg-protocol/`,
`crates/gregg-host/`, `crates/gregg-update/src/exec.rs`, `greggd`'s control
socket, `greggd/src/startup/install.rs`, and `greggd/src/uninstall.rs`
**unreviewed — not clean**.

## Objective

Audit the surfaces the previous pass never reached, fix every defect confirmed
by hand against source, and close the gap honestly — stating what still cannot
be verified from a Linux host rather than letting silence read as a pass.

As in Plan 182, no mechanical gate was failing at `aa321dd`: `cargo test`,
`cargo clippy -D warnings`, `cargo fmt`, `shellcheck`, and `pytest` were all
green. Every finding below is logical.

## Findings and required behavior

### 1. The Cargo fallback skipped client-daemon finalization (`packaging/install.sh`)

`finalize_gregg_install` was called on exactly three paths: a source-only-host
Cargo fallback, and the prebuilt release-asset success. It was **absent** from
the HTTP-404 Cargo fallback.

A 404 is a designed, reachable outcome — `only HTTP 404 falls back to cargo
install` — but the client daemon runs *from the installed executable*. So on
that path the daemon state was captured (`capture_user_local_clientd`, common
to all branches) and then never transitioned: on Unix the replacement installs
a new inode and the running client daemon keeps executing the replaced-out
image indefinitely, and no client startup entry is ever registered. The
identical `greggd` operation on the same branch did reach its finalization,
which is what made the omission an asymmetry rather than a policy.

Required: the 404 Cargo fallback reaches the same shared finalization as every
other acquisition path.

### 2. The Windows Cargo fallback skipped the client-daemon transition (`packaging/install.ps1`)

The mirror-image defect. `Invoke-CargoFallback` called
`Register-UserLocalClientdStartup` but not the `daemon stop` / `daemon restart`
transition that the prebuilt path performs, contradicting the code's own
comment that "prebuilt and staged-Cargo candidates share the same replacement
and daemon post-install finalization path".

Required: both acquisition paths share one implementation of the transition, so
a startup entry is never registered for a replacement whose running daemon was
left un-transitioned.

### 3. A `cargo uninstall` timeout was reported as a `cargo install` timeout (`gregg-update`)

`run_command_with_timeout_for_cargo` serves both subcommands but hardcoded the
word `install` in its messages. A user in the uninstall path is told the update
fallback timed out.

Required: the message names the operation actually run.

### 4. A post-exit drain timeout was reported as a killed child (`gregg-update`)

The post-exit settle bound expiring (`POST_EXIT_DRAIN_SETTLE`) is also
`ErrorKind::TimedOut`, so `map_capture_error` reported "curl timed out and was
killed". Nothing was killed — the child had already exited — and the real cause
(an inherited writer holding the pipe open) was hidden. The sibling download
path already reported it truthfully.

Required: keep the real cause; the kill wording stays for a genuine capture
deadline.

### 5. The scheduler validator's empty-name check was dead code (`gregg-protocol`)

```rust
if job.name.is_empty() {
    check_text(violations, &format!("{field}.name"), &job.name, 0);
}
```

`check_text` compares `value.len() > max`, and `"".len() > 0` is never true, so
the branch could not push anything. An empty job name therefore passed
validation, while the history route had no empty check at all. The client keys
cron rows by job name and tolerates an empty one by skipping the row, so
records the daemon published would be silently dropped.

Required: reject an empty name on both routes, with its own violation kind —
an empty string is within the length bound, not over it (mirroring
`EmptyDriveName` in the v2 validator).

### 6. A manager install could register a service running a different binary (`greggd`)

The systemd `ExecStart` and launchd `ProgramArguments[0]` name a fixed
canonical path (`/usr/local/bin/greggd`); only the config is parameterized. But
ownership everywhere else — `uninstall`, `restart` — is proved against the
*exact invoked* executable. So an install invoked from anywhere else wrote an
artifact that runs a different binary and that the same product then classified
as `Foreign`: `uninstall --dry-run` reports "foreign installation preserved" and
`restart` refuses with "foreign systemd registration owns the selected config".

The code already carried the intended check in a comment ("if current exe
exists but not at standard path, give actionable error") that was never
implemented.

Required: refuse the install. This is the only outcome where the writer and the
classifier agree.

### 7. `crontab` was the one manager call with no timeout (`greggd`)

`crontab -l` and `crontab -` called `Command::output()` / `child.wait()`
directly, while every other manager execution went through
`run_bounded_command(..., MANAGER_COMMAND_TIMEOUT)`. A blocked `crontab -l`
(locked/NFS spool, hung cron daemon) hangs an operator command forever. The
4 MiB cap was also applied *after* `output()` had buffered everything.

Required: both calls join the same bounded allowlist. `crontab -` needs stdin,
so the shared helper gained a bounded stdin variant; the 4 MiB cap is now
enforced by the reader before buffering rather than by a post-hoc length
check.

### 8. An unparseable SCM image path was classified `Absent`, not `Unknown` (`greggd`)

The other three classifiers (systemd `ExecStart`, launchd plist, cron block)
map unreadable-or-unparseable to `Unknown`. The Windows one mapped a
registration whose image path could not be parsed unambiguously to `Absent` —
which claims there is nothing to preserve, so `--dry-run` printed nothing about
a service that survives the teardown, and nothing blocked.

Required: classify it `Unknown`, matching the other three and the explicit
contract bullet that SCM query uncertainty blocks mutation.

### 9. Copy-paste instructions printed unquoted paths (`greggd`)

`startup instructions` printed `sudo {exe} startup install --method …` and an
unquoted `--config`, while the `PermissionDenied` path printed the very same
command through `elevated_command`, which quotes. A path with a space re-parses
as several arguments, so the "exact rerun command" does not run. The systemd
uninstall rerun hint had the same omission. A test already pinned the quoting
contract for `elevated_command`; only the instructions renderer ignored it.

Required: quote through the same helper.

## Verification

Every behavioral fix carries a regression test that was executed against the
pre-fix logic and observed failing with the recorded symptom.

| Test | Observed pre-fix failure |
| --- | --- |
| `scripts/tests/test-install-rerun.sh` — staged Cargo client transition + startup registration | `FAIL`; daemon log showed only `gregg daemon status` — no `stop`, no `restart`, no `startup install` |
| `exec::tests::cargo_timeout_reports_the_operation_actually_run` | `an uninstall timeout must name the uninstall operation: cargo fallback failed: cargo install timed out after 0s` |
| `exec::tests::post_exit_drain_expiry_is_not_reported_as_a_capture_kill` | reported a kill that never happened |
| `validate_scheduler::tests::empty_job_names_are_rejected_on_both_routes` | `expect_err` on an empty-name document that validated clean |
| `startup::install::tests::a_manager_install_refuses_a_binary_it_would_not_itself_run` | `expect_err` on a manager install that accepted a foreign binary |
| `startup::install::tests::instructions_quote_the_executable_and_config_they_print` | `systemd install line is unquoted` |
| `startup::process::tests::bounded_command_with_stdin_delivers_input_and_still_bounds_the_child` | `expect_err` on a slow child that was never bounded (and the pre-existing `bounded_manager_command_captures_stderr_and_kills_on_timeout` failed alongside it) |
| `startup::process::tests::a_child_that_exits_early_reports_its_status_not_the_broken_pipe` | panicked returning the broken pipe instead of the child's status |

The EPIPE test needed two rewrites before it detected anything: a small write
is absorbed by the pipe buffer before the child can exit, so the first version
passed against mutated code. It now writes 512 KiB into a child that never
reads, which forces the block-then-EPIPE deterministically.

The SCM image-path classification (finding 8) has **no regression test**: it
lives inside `#[cfg(target_os = "windows")]` and cannot execute on a Linux host.

## Gates

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, 0 warnings |
| `cargo test --workspace --all-targets --all-features` | pass, 0 failed |
| `shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh` | pass, zero findings |
| `bash scripts/tests/test-install-rerun.sh` | pass, 112 assertions, 0 fail |
| `python3 -m pytest scripts/tests -q` | pass |

## Out of scope

Reported and deliberately **not** changed:

- **The whole-crontab read-modify-write race.** `install_cron` and
  `uninstall_cron_for_config` re-list the entire table and pipe the rebuilt
  version to `crontab -`. A concurrent write to the same account's crontab is
  silently lost. This is inherent to the `crontab -` interface, which offers no
  locking or compare-and-swap. Fixing it means changing the artifact format or
  abandoning `crontab`, both of which are product decisions.
- **macOS `--purge` ordering.** A root-owned `/var/log/greggd.log` is removed
  after the config is already deleted and before the executable self-delete, so
  an unprivileged `--purge` leaves a half-finished uninstall. Preflighting it
  would reorder a macOS-only destructive sequence that cannot be executed or
  tested from this host.
- **`gregg-host` / `gregg-protocol` non-Linux runtime behavior.** See below.

## Coverage statement

This pass closes the Rust-side gap Plan 182 disclosed, with one explicit
exception. Newly audited and **reviewed clean** by line-by-line reading:
`gregg-update` (all eight modules), `gregg-protocol` (all modules including the
36-kind v2 validator), `gregg-host` (all four platform modules plus `rate.rs`
and `DriveRefreshCache`), `greggd`'s control socket, `service/windows.rs` SCM
dispatcher and image-path parser, `status.rs`, `net.rs`, and the packaging
installers.

**Not verified, and not verifiable from a Linux host:** the macOS, Windows, and
FreeBSD FFI surfaces were compiled by cross-check and read line by line, but
they cannot *execute* here. Byte-offset constants (`DEVSTAT_READ_INDEX`, the
`if_data` prefix), `GetIfTable2` / `host_statistics64` / `ifmib` struct layouts,
`CallNtPowerInformation` / `RtlGetVersion` behavior, and the Windows commit
memory / topology math need a native run. `packaging/install.ps1` likewise
cannot execute here — findings 2 and 9 are correct by construction and reading,
not by observation.

Per `AGENTS.md`, a green local cross-check is not closure evidence for Windows:
**only a native Windows CI run is authority there.**

## Closure record

Completed on `main`. See the commit for this plan for the final gate totals.