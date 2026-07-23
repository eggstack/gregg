# Phase 5: daemon configuration, lifecycle, and packaging

## Objective

Make `greggd` deployable as a native system service on Linux and macOS. Implement configuration discovery and atomic mutation, service lifecycle commands, installation assets, privilege handling, and repeatable packaging without changing the daemon’s foreground process model.

Version 1 supports systemd on Linux and launchd on macOS. Other init systems and per-user agents are outside scope unless later required by deployment evidence.

## CLI contract

Implement:

```text
greggd run [--config PATH]
greggd start
greggd stop
greggd restart
greggd croncheck
greggd host ADDRESS
greggd port PORT
```

Optional diagnostic commands such as `status`, `config-path`, or `validate` may be added only if they materially simplify support and do not alter the required contract.

Command behavior:

- `run` loads validated config and remains in the foreground.
- `start`, `stop`, and `restart` delegate to the native service manager.
- `croncheck` is idempotent: if the service is active, exit success without action; if inactive, start it; if service state cannot be determined, return a nonzero diagnostic.
- `host` and `port` load the current config, update one value, validate the complete result, atomically persist it, then restart the service.
- Failed validation or failed persistence must not restart the active service.

If the original desired semantics for `croncheck` are later clarified as “check only,” rename or add a distinct `is-running` command rather than leaving an operationally misleading implementation.

## Configuration schema

Recommended daemon config:

```toml
name = "Deadpool"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
```

Keep version 1 fields narrow. Add defaults in one location and serialize canonical TOML. Preserve comments only if using `toml_edit` and doing so remains reliable; correctness is more important than comment retention for command-driven single-field edits.

Validate:

- Display name is nonempty after trimming and within a documented length bound.
- Host is a valid IPv4/IPv6 address under the version-1 bind policy.
- Port is in `1..=65535`.
- Sample interval falls within supported bounds.
- Staleness threshold exceeds or meaningfully relates to sample interval.

Unknown fields should produce a deliberate warning or error according to a documented forward-compatibility policy. Silent typo acceptance is undesirable for system-service configuration.

## Platform config locations

Default system paths:

```text
Linux: /etc/gregg/greggd.toml
macOS: /Library/Application Support/gregg/greggd.toml
```

`--config PATH` overrides discovery for development, tests, containers, or nonstandard deployments. Lifecycle commands should use the installed service definition’s path rather than guessing a different override.

Provide a platform-specific development config path only if needed; do not make service behavior depend on the invoking user’s home directory.

## Atomic writes

Configuration mutation must:

1. Resolve and validate the destination directory.
2. Serialize the complete updated config.
3. Write a uniquely named temporary file in the same directory.
4. Set intended ownership/permissions where the installer controls them.
5. Flush file contents and, where practical, directory metadata.
6. Rename atomically over the destination.
7. Reopen and parse the final file as a verification step.
8. Restart only after successful verification.

On failure, remove the temporary file when safe and leave the prior config intact. Tests must simulate interruption/failure before rename and prove the old file remains valid.

## Service abstraction

Create an internal trait or enum-backed adapter:

```rust
trait ServiceManager {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn restart(&self) -> Result<()>;
    fn is_active(&self) -> Result<bool>;
}
```

Platform modules:

```text
service/systemd.rs
service/launchd.rs
```

External command invocation is acceptable for service-manager control because `systemctl`/`launchctl` are the native administrative interfaces. Use fixed executable paths or careful resolution, fixed argument lists, no shell, inherited or captured stderr, and clear exit-status handling.

Do not use external commands for metrics collection.

## Linux systemd packaging

Add a unit template under `packaging/systemd/greggd.service`. It should:

- Use `Type=simple`.
- Execute `greggd run --config /etc/gregg/greggd.toml`.
- Restart on unexpected failure with bounded delay.
- Stop cleanly on SIGTERM.
- Avoid unnecessary capabilities and writable filesystem access.
- Run as a dedicated system user if installation complexity remains reasonable, or document the chosen unprivileged account model.
- Permit reading procfs and binding the configured unprivileged default port.
- Set conservative hardening options only after testing on target distributions; do not add options that break ARM boards or older supported systemd versions without evidence.

Installer behavior should create directories, install the binary/config/unit, reload systemd, enable/start only with explicit user intent, and be idempotent.

Test uninstall/upgrade behavior sufficiently that replacing the binary does not destroy configuration.

## macOS launchd packaging

Add `packaging/launchd/com.eggstack.greggd.plist`. Use a system LaunchDaemon because collection should not depend on an interactive login.

The plist should:

- Execute the absolute `greggd` path with `run --config ...` arguments.
- Use `RunAtLoad` if installation semantics expect immediate service start.
- Use `KeepAlive` or restart policy restricted to abnormal termination rather than tight restart loops on invalid config.
- Route stdout/stderr to documented log paths or rely on unified logging behavior consistently.
- Set a working directory only if required.
- Avoid running as root after startup if a dedicated account is practical and tested.

Use modern `launchctl bootstrap`, `bootout`, and `kickstart` flows appropriate to supported macOS versions. Keep command construction centralized and testable.

Paths containing spaces must be passed as argument-array elements, not shell-quoted strings.

## Privilege model

System installation and mutation of system config/service state generally require administrator privileges. The binary must not silently invoke `sudo` or prompt unexpectedly inside library code.

Preferred behavior:

- Detect permission errors.
- Print the exact command or installation step requiring elevation.
- Let package managers/installers or users invoke privileged operations explicitly.
- Keep `greggd run --config <writable temp path>` usable unprivileged for development.

Document network exposure: the default `0.0.0.0` bind makes metrics visible to reachable peers. Provide `greggd host 127.0.0.1` as the documented SSH-tunnel-only option.

## Packaging and release artifacts

Prepare:

```text
packaging/
├── systemd/greggd.service
├── launchd/com.eggstack.greggd.plist
├── install-linux.sh
├── install-macos.sh
└── README.md
```

Scripts must use strict error handling, validate architecture/binary paths, avoid downloading unverified content implicitly, and be safe to rerun. Package-manager formulas are optional post-version-1 work unless needed for release distribution.

## Tests

Implement command-level tests with fake service-manager adapters. Verify:

- Start/stop/restart dispatch.
- `croncheck` active and inactive branches.
- Failed status query.
- Host and port validation.
- Atomic-write success and injected failures.
- Restart occurs only after verified persistence.
- Permission-denied diagnostics.
- Paths with spaces on macOS.
- Service command argument construction without shell interpolation.

Native installation smoke tests should run in disposable Linux and macOS environments where permissions allow. CI without service-manager access may validate generated files and adapter command construction but cannot be the sole deployment evidence.

## Acceptance criteria

Phase 5 is complete when:

1. All required daemon CLI commands exist with stable help and meaningful exit codes.
2. `greggd run` remains foreground-only and uses no PID file or self-daemonization.
3. Linux and macOS default config paths are correct and overridable for development.
4. Host/port mutations are atomic, fully validated, and restart only after successful verification.
5. `croncheck` behavior is documented, idempotent, and tested.
6. systemd and launchd adapters use fixed argument arrays and report native failures clearly.
7. Installation assets are idempotent and preserve existing valid configuration on upgrade.
8. A disposable Linux host can install, start, stop, restart, and reboot with `greggd` returning.
9. A disposable macOS host can bootstrap, stop, restart, and reboot/login-cycle with the LaunchDaemon returning.
10. Service definitions run the exact packaged binary/config paths and handle paths with spaces.
11. The daemon can also run unprivileged with a temporary config for development/testing.
12. `cargo package -p greggd` contains required templates/docs or has a documented external packaging strategy and installs successfully from the package.

## Handoff to client phases

Document the final endpoint URL, status behavior, service commands, and example deployment. Client work should assume only the read-only protocol, not native service access.
