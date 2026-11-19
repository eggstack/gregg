# Plan 194: Make the advisory policy truthful and run it in release preflight

Status: proposed (not started).

Source: 2026-11-17 security audit, finding 10.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-193.

## Problem

`cargo deny check advisories` currently exits 1 on this tree:

- `chrono 0.4.45` — RUSTSEC-2026-0543, unmaintained;
- `chrono-tz 0.10.4` — RUSTSEC-2026-0555, unmaintained;
- `deny.toml:7` ignores `RUSTSEC-2026-0185`, which matches no crate, so the
  policy also reports an unused ignore.

There is no known vulnerability, unsoundness, or yanked-crate finding. chrono
and chrono-tz are used for civil-time schedule handling (`greggd`) and local
time rendering (client); replacing them is a real migration, not a policy
edit, and is not part of this plan.

`cargo-deny` was deliberately removed from ordinary CI (Plan 046), so nothing
gates advisories today. `deny.toml` exists but is not exercised by
`./scripts/check-local.sh --release` either, which means the policy can rot
without anyone noticing — as it has.

## Decision

1. Make `deny.toml` truthful: remove the stale ignore and record the two
   unmaintained crates as explicit, reasoned exceptions. Do **not** silently
   drop the advisory category or switch `unmaintained` to `allow`.
2. Add an advisory step to the *release* preflight
   (`./scripts/check-local.sh --release`), which is explicitly requested and
   not ordinary CI. If `cargo-deny` is installed, findings fail the preflight;
   if it is not installed, print a skip notice and continue (Plan 046's
   requirement that the tool remain optional).
3. Record the accepted risk and its review trigger in `RELEASING.md`.

Replacing chrono/chrono-tz with `jiff`/`time` is a separate future plan; this
plan must not pretend the unmaintained status is fixed.

## Scope

In scope: `deny.toml`, one optional step in `scripts/check-local.sh`,
`RELEASING.md`, and a short `CHANGELOG.md`/docs note if the repository records
dependency policy elsewhere. Out of scope: replacing the crates, adding a CI
job or GitHub Action, changing `[licenses]`/`[bans]`/`[sources]`, and any
runtime behavior.

## Implementation

1. `deny.toml`
   - Remove `ignore = ["RUSTSEC-2026-0185"]`; the advisory no longer matches
     the tree.
   - Add the two unmaintained advisories to `ignore`, each with a comment
     stating: crate, why it is still needed, that no patched release exists,
     and the review trigger (a maintained successor adoption, a vulnerability
     advisory, or the next major release planning).

     ```toml
     ignore = [
       # RUSTSEC-2026-0543: chrono has no maintained release. Used only for
       # civil-time schedule math; revisit with the chrono -> jiff/time plan.
       "RUSTSEC-2026-0543",
       # RUSTSEC-2026-0555: chrono-tz has no maintained release. Time-zone
       # conversion for scheduler display; same review trigger.
       "RUSTSEC-2026-0555",
     ]
     ```

   - Keep `unmaintained = "workspace"` and `yanked = "warn"`.

2. `scripts/check-local.sh`
   - In the `--release` tier only, before or after the existing package
     checks, add a step that runs when `command -v cargo-deny` succeeds:

     ```bash
     step "cargo deny check advisories"
     run_or_fail cargo deny check advisories
     ```

     and when it does not:

     ```bash
     echo "skip: cargo-deny not installed; advisory check not run" >&2
     ```

   - Do not add it to the default tier and do not add a CI job.

3. `RELEASING.md` step 3 (local release preflight) gains one sentence: the
   preflight runs `cargo deny check advisories` when `cargo-deny` is
   installed, and the current exceptions are the two unmaintained crates
   recorded in `deny.toml`. Add the install hint
   (`cargo install cargo-deny --locked`).

4. `CHANGELOG.md`: a maintenance entry only if the repository records
   dependency-policy changes there; otherwise skip it rather than padding the
   changelog.

## Tests and evidence

- `cargo deny check advisories` exits 0 on the updated `deny.toml`.
- `cargo deny check` (full policy) exits 0, or any remaining license/ban
  finding is recorded rather than ignored.
- With the tool installed, `./scripts/check-local.sh --release` runs the step
  and fails on a knowingly bad ignore (verified by temporarily adding a
  nonexistent advisory id and observing the failure, then reverting).
- With the tool absent, the script prints the skip notice and still completes
  the rest of the release tier.
- The default `./scripts/check-local.sh` is unchanged.

## Acceptance criteria

- [x] `cargo deny check advisories` exits 0.
- [x] `deny.toml` has no unused ignore; each retained ignore has a reason and
      a review trigger.
- [x] The advisory policy is visible in the release preflight rather than
      existing as an unexercised file.
- [x] `cargo-deny` remains optional and absent from ordinary CI (Plan 046).
- [ ] `./scripts/check-local.sh --release` passes with the tool installed and
      prints a skip notice without it.
- [x] `shellcheck -x scripts/*.sh` is clean.

## Verification

```bash
cargo deny check advisories
cargo deny check
./scripts/check-local.sh --release
shellcheck -x scripts/*.sh
```

## Risks

- Ignoring an unmaintained advisory is a real, accepted risk: no security
  patches arrive for a crate with no maintainer. The mitigation is the
  recorded review trigger and keeping the crates on paths that do not parse
  untrusted input. If either crate gains a vulnerability advisory, the ignore
  must not be extended; that becomes an immediate migration.
- The release preflight gets slower by one tool invocation when installed.
  That is the point of the release tier.

## Closure record

### What shipped

- `deny.toml` no longer ignores `RUSTSEC-2026-0185`; `RUSTSEC-2026-0543`
  (`chrono`) and `RUSTSEC-2026-0555` (`chrono-tz`) are recorded with the crate,
  why it is still needed, the absence of a patched release, and the review
  trigger (maintained successor, a new vulnerability advisory, or next major
  release planning). `unmaintained`/`yanked` and the other sections are unchanged.
- `scripts/check-local.sh` runs `cargo deny check advisories` in the `--release`
  tier only, and prints `skip: cargo-deny not installed; advisory check not run`
  when the tool is absent. The default tier is untouched.
- `RELEASING.md` states that the preflight runs the advisory check, names the two
  recorded exceptions, and gives the `cargo install cargo-deny --locked` hint.

### Verification

- `cargo deny check advisories` — exit 0, `advisories ok`, no warnings, so no
  unused ignore remains.
- `cargo deny check` (full policy) — exit 0: advisories, bans, licenses, sources
  all ok. Only the non-failing `multiple-versions` warn tree.
- Fail path proven: temporarily removing one real ignore produced exit 1 with
  `error[unmaintained]` and `local check failed: cargo deny check advisories`.
  Skip path proven by running the extracted hunk with `~/.cargo/bin` off `PATH`:
  the skip notice printed and the hunk exited 0.
- `bash -n scripts/check-local.sh` and `shellcheck -x scripts/check-local.sh` —
  clean.

### Not verified here

- `./scripts/check-local.sh --release` was not run end to end: the tier requires a
  clean tree, and the default tier is red on this host for pre-existing failures
  in code this plan does not touch (recorded in the sibling closure records). The
  new step's body was exercised in isolation instead.
- On the installed cargo-deny (0.20.2) an unused or unknown ignore is a
  **warning**, not an error, so the plan's "prove it with a nonexistent id" probe
  does not fail on this version; the real-advisory probe above was used instead.
  Nothing currently enforces "no unused ignore" going forward. Adding
  `-D advisory-not-detected,unknown-advisory` would, but that flag is not
  available on every cargo-deny version, so it was left out deliberately.
