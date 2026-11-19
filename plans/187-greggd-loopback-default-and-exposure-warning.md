# Plan 187: Bind `greggd` to loopback by default and make exposure explicit

Status: complete. See the closure record at the end.

Source: 2026-11-17 security audit, finding 1.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-186.

## Problem

`Config::default()` binds `0.0.0.0` (`crates/greggd/src/config.rs:173`), and
every installer writes that value into its generated config
(`packaging/install-linux.sh:103`, `packaging/install-macos.sh:102`,
`packaging/install.ps1:170`, `packaging/install-windows.ps1:104`).
Configs written by `greggd startup install` inherit the same default through
`ensure_config_preserved` (`crates/greggd/src/startup/install.rs:389-401`).

The listener has no authentication and no TLS, and
`GET/HEAD /v2/scheduler/history` returns bounded stdout/stderr tails for
operator-configured jobs (`crates/gregg-protocol/src/scheduler.rs:304-315`,
`:398-430`; routes at `crates/greggd/src/server/mod.rs:1096-1102`). Nothing
warns at startup when the bound address is unspecified.

The project's documented position is that `greggd` is a private-network tool
and that operators must firewall it (`SECURITY.md:23-34`,
`docs/daemon.md:169-171`). A wildcard default contradicts that position in
practice: a laptop that joins an untrusted network, or a cloud host with a
public interface, is exposed without the operator making a choice.

## Decision

- The default bind becomes `127.0.0.1`.
- LAN exposure remains supported and is an explicit configuration edit
  (`host = "0.0.0.0"` or a concrete interface address).
- Any non-loopback bind logs one startup warning naming the address and the
  fact that the API is unauthenticated.
- Existing configuration files are never rewritten. An operator who already
  has `host = "0.0.0.0"` keeps that explicit choice after upgrade.

This keeps fleet monitoring possible with one edit while making exposure an
affirmative decision. It does not add authentication, TLS, or rate limiting;
those remain outside the design (`SECURITY.md`).

## Scope

In scope:

- `Config::default()` and its doc comment;
- one pure exposure predicate and one startup warning call;
- the four installer-generated default configs;
- user-facing docs that show or describe the default;
- a packaging contract test that locks the new default.

Out of scope: rewriting existing config files, changing `gregg` endpoint
handling, changing `greggd status`/`configprint` output, auth/TLS, rate
limiting, and any wire-format change.

## Implementation

1. `crates/greggd/src/config.rs`
   - Change `Config::default().host` to `IpAddr::V4(Ipv4Addr::LOCALHOST)`.
   - Update the `host` field doc to say the default is loopback and that a
     wildcard/concrete address exposes an unauthenticated API.

2. Add a pure predicate in `config.rs`:

   ```rust
   pub(crate) fn bind_exposure_warning(host: IpAddr) -> Option<&'static str>
   ```

   - `None` for `is_loopback()` addresses (`127.0.0.0/8`, `::1`).
   - `Some(...)` for unspecified (`0.0.0.0`, `::`) and for every concrete
     non-loopback address, including private ranges. The message names the
     address, says the API has no authentication/TLS, and points at the
     firewall or a loopback bind.

3. Call it once in the daemon startup path after config validation and before
   the listener binds (`crates/greggd/src/run.rs`, next to the existing
   startup logging). Emit exactly one `tracing::warn!` per process; do not
   warn per sample or per request.

4. Update the installer default config blocks to `host = "127.0.0.1"` in
   `packaging/install-linux.sh`, `packaging/install-macos.sh`,
   `packaging/install.ps1`, and `packaging/install-windows.ps1`. Keep the rest
   of each block byte-identical.

5. Leave `ensure_config_preserved` alone: it writes `Config::default()` and
   therefore picks up the new default with no extra code.

6. Docs:
   - `docs/daemon.md:22` example changes to `127.0.0.1`, with a short "LAN
     monitoring" note showing `0.0.0.0` as the explicit opt-in.
   - `README.md` security section states the loopback default.
   - `SECURITY.md` scope keeps the no-auth/no-TLS statement and adds that the
     default bind is loopback.
   - `docs/installation.md`: check the first-run/config-creation text for a
     stale wildcard claim and update it.
   - `CHANGELOG.md`: one entry under the unreleased section describing the
     default change and that existing configs are unchanged.

7. Add `scripts/tests/test_security_contracts.py` (new pytest file) with a
   case that reads the four installer files and asserts each generated
   default block contains `host = "127.0.0.1"` and does not contain
   `host = "0.0.0.0"`. This is a packaging-text contract, not a shell-parse
   test; it exists so a later copy of the old block fails CI.

## Tests

- `default_config_binds_loopback` — `Config::default().host.is_loopback()`.
- `default_config_serializes_loopback` — `to_toml()` contains
  `host = "127.0.0.1"`, then round-trips.
- `exposure_warning_is_silent_for_loopback` — `127.0.0.1`, `::1`.
- `exposure_warning_fires_for_unspecified_and_lan` — `0.0.0.0`, `::`,
  `192.168.1.10`, `fd00::10`.
- Existing config tests keep using explicit `0.0.0.0` fixtures where the
  wildcard is the subject; those are not default assertions and must not be
  changed.
- `scripts/tests/test_security_contracts.py::test_installer_default_binds_loopback`.

## Acceptance criteria

- [x] `Config::default()` binds `127.0.0.1`, and a freshly installed daemon
      with no pre-existing config listens only on loopback.
- [x] A config that explicitly sets `host = "0.0.0.0"`, a concrete LAN
      address, or `::` still binds as before.
- [x] A non-loopback bind logs exactly one warning at startup; a loopback bind
      logs none.
- [x] Existing config files are not rewritten by startup install, update, or
      uninstall.
- [x] All four installers create `host = "127.0.0.1"`; the contract test
      fails if any reverts.
- [x] Docs no longer present the wildcard as the default.
- [ ] `cargo fmt --all -- --check`, `./scripts/check-local.sh`,
      `cargo test -p greggd --all-targets --all-features`, and
      `python3 -m pytest scripts/tests -q` are clean.

## Verification

```bash
cargo test -p greggd --all-features -- config
./scripts/check-local.sh
python3 -m pytest scripts/tests -q
```

Manual, on one host:

1. Install with no pre-existing config and confirm `greggd configprint`
   prints `127.0.0.1:11310`, `ss -ltn` shows only loopback, and the daemon
   warns once when `host` is edited to `0.0.0.0`.
2. Confirm an existing `0.0.0.0` config is preserved byte-for-byte across
   `greggd startup install`.

## Risks and rollback

- A fleet operator who relies on the wildcard default and never wrote a config
  will stop receiving samples after upgrade. That is the intended behavior
  change, but it must be prominent in `CHANGELOG.md` and the release notes,
  with the one-line fix (`host = "0.0.0.0"` and restart).
- Rollback is a one-line revert of `Config::default()` plus the installers;
  no data migration is involved.
## Closure record

### What shipped

- `Config::default().host` is `127.0.0.1`, and the `host` field doc states that
  the API is unauthenticated and that a wildcard or LAN bind is an explicit
  choice.
- `config::bind_exposure_warning(host: IpAddr) -> Option<&'static str>` returns
  `None` for loopback and one of two distinct messages otherwise (wildcard vs
  concrete address). `run.rs` calls it once, before the listener binds, and
  attaches the real address as a structured `host` field rather than
  interpolating it into the message.
- All four installers generate `host = "127.0.0.1"`.
- `docs/daemon.md` gained a "LAN monitoring" subsection showing `0.0.0.0` as the
  opt-in; `README.md` and `SECURITY.md` state the loopback default;
  `CHANGELOG.md` records the change and the one-line fix for a LAN fleet;
  `architecture/greggd-daemon.md` documents the default and the single warning.
- `scripts/tests/test_security_contracts.py::test_installer_default_binds_loopback`
  locks the four installer blocks so a later copy of the old default fails CI.
- One existing test did depend on the old default:
  `cli::native_tests::config_address_preserves_wildcards_and_formats_ipv6` used
  `Config::default()` as its wildcard fixture. It now sets `host = "0.0.0.0"`
  explicitly, because the wildcard is that test's subject; its assertion is
  unchanged.

### Verification

- `cargo test -p greggd --all-features -- config::tests` — the five new tests
  pass (`default_config_binds_loopback`, `default_config_serializes_loopback`,
  `exposure_warning_is_silent_for_loopback`,
  `exposure_warning_fires_for_unspecified_and_lan`,
  `explicit_wildcard_config_still_parses`).
- `python3 -m pytest scripts/tests/test_security_contracts.py -q` — 4 passed,
  the installer-default case among them. The guard was checked by feeding a
  reverted block to the predicate: it fails.
- `cargo fmt --all -- --check` — clean.

### Not verified here

- `./scripts/check-local.sh` is red on this host for pre-existing reasons only.
  `clientd::ipc::tests::a_config_inside_the_temp_directory_yields_one_location`
  fails identically on a pristine `fabcaf2` checkout (macOS `TMPDIR` is not
  `/tmp`); `scheduler::tests::fleet_scaling_*_endpoints` fails only inside a
  loaded parallel run and passes in isolation; and ten `greggd` tests
  (`control::tests::*`, `uninstall::tests::purge_config_*`) fail identically at
  `fabcaf2` because `primary_control_path` returns `None` when the config sits
  under this host's long `TMPDIR`, past the Unix socket path limit.
  `cargo test -p greggd --all-features` is therefore 524 passed / 10 failed both
  before and after this plan, and the only test it changed behaviour for is the
  wildcard fixture noted above.
- No live install was performed, so "a freshly installed daemon listens only on
  loopback" follows from `Config::default()` plus the installer text rather than
  from an observed `ss -ltn`.
