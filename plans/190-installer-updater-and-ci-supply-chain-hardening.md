# Plan 190: Harden installer cleanup, HTTPS redirects, and CI action pinning

Status: proposed (not started).

Source: 2026-11-17 security audit, findings 5 and 11.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-189.

## Problem

Three small supply-chain defects share one theme: a security property is
stated in a comment but not enforced by the construct.

### A. The installer's EXIT trap re-parses `mktemp` output

`packaging/install.sh:755` and `:912` build cleanup with double-quote
interpolation at definition time:

```bash
trap "rm -rf \"$tmpdir\"" EXIT
```

The resulting string is parsed again by the shell at exit, so a `TMPDIR`
containing `$(...)` or backticks executes a command when the installer runs as
root. I confirmed the trap-body re-parse in an isolated shell. GNU
`mktemp -d` honours `TMPDIR`, so the precondition is real on Linux (macOS
`mktemp` did not honour it in my check). The `# shellcheck disable=SC2064`
comment marks the construct as deliberate; it is deliberate about *expansion
time* but not about quoting.

### B. HTTPS downloads are not pinned across redirects

`crates/gregg-update/src/exec.rs:180-218, 240-260, 340-374` and
`packaging/install.sh:923, 957` pass `--proto '=https'` with `-L`. `--proto`
constrains the initial request only; curl's default `--proto-redir` still
permits an HTTP hop. The checksum is fetched over the same channel, so the
practical gain is defence in depth rather than a new trust boundary — but the
comment claims a pin the command does not implement.

### C. Workflow actions are pinned by mutable tags

`.github/workflows/*.yml` use `actions/checkout@v4`, `actions/cache@v4`,
`actions/upload-artifact@v4`, `actions/download-artifact@v4`,
`dtolnay/rust-toolchain@stable`, `dtolnay/rust-toolchain@master`, and
`vmactions/freebsd-vm@v1.5.8`. A moved tag changes what runs with the
workflow's token; `release-binaries.yml` holds `contents: write`.

## Scope

In scope: the two trap sites, every `--proto '=https'` curl invocation in the
audited scripts and updater, the workflow `uses:` pins, and one static
contract test that locks all three. Out of scope: adding artifact signing
(explicitly considered and not part of this plan), changing the release
workflow's permissions, and replacing `cargo-deny`'s manual role (Plan 194).

## A. Replace the interpolated traps

Use one global list plus a function, and keep bash 3.2 compatibility (macOS
ships 3.2; `set -u` with an empty `"${arr[@]}"` is an error there):

```bash
CLEANUP_PATHS=()
cleanup_paths() {
  if [ "${#CLEANUP_PATHS[@]}" -gt 0 ]; then
    local path
    for path in "${CLEANUP_PATHS[@]}"; do
      [ -n "$path" ] && rm -rf -- "$path"
    done
  fi
}
trap cleanup_paths EXIT
```

- Register with `CLEANUP_PATHS+=("$stage_root")` / `("$tmpdir")`.
- Where a caller currently runs `trap - EXIT` to take over cleanup
  (`install.sh:788, 932, 985`), clear the list as well
  (`CLEANUP_PATHS=()`) before the existing `trap - EXIT`, so ownership moves
  explicitly rather than by re-expansion.
- Remove the `SC2064` suppressions and the "expand now so trap sees value"
  comments; the new comment must say the trap body is a fixed command and the
  path is data.
- Model the construct on `scripts/release-install-zig.sh:33`, which already
  uses a single-quoted trap.

Do not change the `rm -rf` target semantics: each entry is a `mktemp -d`
directory owned by the installer.

## B. Pin redirects to HTTPS

Add `--proto-redir '=https'` next to every `--proto '=https'`:

- `crates/gregg-update/src/exec.rs`: `probe_http_code`, the crates.io
  metadata capture, and `download_file` (https branch only; the plain-http
  branch exists for deterministic local fixtures and must stay usable).
- `packaging/install.sh`: the asset download and the checksum download.
- `scripts/release-install-zig.sh:34` and `scripts/release-preflight.sh:156`
  as a sweep; both already pin the initial protocol.

Update the comments so they state the actual guarantee: initial request and
every redirect are HTTPS-only. Keep `--tlsv1.2`.

## C. Pin workflow actions to commit SHAs

- Resolve each `uses:` tag to a 40-hex commit SHA with
  `gh api repos/<owner>/<repo>/git/ref/tags/<tag>` (or `git ls-remote`), and
  write `uses: owner/repo@<sha> # <tag>`.
- Cover every action in `ci.yml` and `release-binaries.yml`, including
  `dtolnay/rust-toolchain@stable` and `@master` (pin the commit they resolve
  to today; the toolchain version still comes from the workflow input and
  `rust-toolchain.toml`).
- Do not change job structure, permissions, or matrix.
- Add a short comment at the top of each workflow: "Actions are pinned to
  commit SHAs; update the SHA and the trailing tag comment together."

Note: `release-binaries.yml` can only be exercised by a tag push or a manual
dispatch, and a dispatch would attach assets to a real release. Verify its
pins with `gh api` (the SHA exists and names the expected tag) and a YAML
parse; the next real release exercises it. `ci.yml` is exercised by the next
ordinary push. Record this evidence boundary in the plan closure.

## D. Lock the three properties with one static test

Add `scripts/tests/test_security_contracts.py` (pytest; the CI scripts job
already runs `python3 -m pytest scripts/tests -q`):

1. `packaging/install.sh` contains `trap cleanup_paths EXIT` and contains no
   `trap "` (double-quoted trap definition).
2. For each audited file, the number of `--proto '=https'` occurrences equals
   the number of `--proto-redir` occurrences:
   `crates/gregg-update/src/exec.rs`, `packaging/install.sh`,
   `scripts/release-install-zig.sh`, `scripts/release-preflight.sh`.
3. Every `uses:` value in `.github/workflows/*.yml` matches
   `@[0-9a-f]{40}` (optionally followed by a `# tag` comment).

The test is intentionally textual; these are contract properties of the
packaging surface that the Rust gates cannot see.

## Acceptance criteria

- [x] No `trap "..."` definition remains in `packaging/install.sh`; cleanup is
      a fixed command over a global path list and works on bash 3.2.
- [x] A `TMPDIR` containing `$(...)` cannot execute a command at exit.
- [x] Every HTTPS curl invocation pins both the initial protocol and redirects
      to HTTPS; plain-http fixture paths still work in tests.
- [x] Every workflow action is pinned to a 40-hex commit SHA with a tag
      comment; CI still runs all jobs.
- [x] The new contract test passes and fails if any of the three properties is
      reverted.
- [x] `shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh`,
      `python3 -m pytest scripts/tests -q`, `cargo fmt`,
      `cargo test -p gregg-update --all-features`, and
      `./scripts/check-local.sh` are clean.
- [ ] One ordinary CI run is green with the pinned `ci.yml` actions.

## Verification

```bash
shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh
python3 -m pytest scripts/tests -q
cargo test -p gregg-update --all-features
./scripts/check-local.sh
gh run list --branch main --limit 3
```

Manual cleanup check: run the installer's download path with a `TMPDIR`
containing `$(touch <marker>)` in a disposable container, then assert the
marker does not exist. Do not run this against a real install prefix.

## Risks

- `--proto-redir` is a curl flag; the project already requires a curl recent
  enough for `--proto` (7.20+), and `--proto-redir` is from the same era. Note
  the minimum in the installer's help if it is not already implied.
- A pinned action SHA stops automatic action updates. That is the point;
  updates become an explicit review step, and the trailing tag comment keeps
  the intent readable.

## Closure record

### What shipped

- `packaging/install.sh`: both interpolated EXIT traps became one fixed
  `cleanup_paths` trap over a `CLEANUP_PATHS` array (bash 3.2 count guard,
  `rm -rf -- "$path"`), re-armed at each registration site and cleared
  (`CLEANUP_PATHS=()`) immediately before each existing `trap - EXIT` handoff.
  The `SC2064` suppressions and the "expand now" comments are gone.
- `--proto-redir '=https'` was added next to every `--proto '=https'`:
  `crates/gregg-update/src/exec.rs` (probe, crates.io metadata, https branch of
  `download_file`), `packaging/install.sh`, `scripts/release-install-zig.sh`,
  `scripts/release-preflight.sh`. The plain-HTTP fixture branch is untouched.
- All 35 `uses:` references in `ci.yml` and `release-binaries.yml` are pinned to
  40-hex commit SHAs with a trailing `# <ref>` comment. `dtolnay/rust-toolchain`
  needed one extra step: its toolchain comes from the `toolchain` input, not the
  ref, so `toolchain: stable` is now explicit at the four sites that previously
  relied on `@stable`.
- `scripts/tests/test_security_contracts.py` locks the trap form, the
  proto/redirect parity, the SHA pins, and (Plan 187) the loopback default.

### Verification

- `shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh` — clean.
- `python3 -m pytest scripts/tests -q` — 65 passed, 2 skipped; the contract file
  is 4 of them, and each detector was shown to fail on a reverted property.
- `cargo test -p gregg-update --all-features` — 51 passed.
- Both workflows parse as YAML; `grep -rn 'uses:'` shows 35/35 pinned values and
  no mutable ref remains.
- Trap construct proof without touching an install prefix: with a path containing
  `$(touch <marker>)`, the new construct removed the path and created no marker,
  while an inline replay of the old double-quoted construct did create it.
  `bash --version` here is 3.2.57, and the empty-array guard was confirmed
  necessary under `set -u`.

### Not verified here

- No workflow run exercises the pins: `ci.yml` next runs on a push, and
  `release-binaries.yml` only on a tag or dispatch (and a dispatch would attach
  assets to a real release). The SHAs were resolved with `git ls-remote` and the
  files parse, but the "one ordinary CI run is green" criterion remains open.
- The installer itself was never executed.
