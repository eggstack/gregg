# Plan 117: Rust 1.89 MSRV and dependency modernization

Status: complete at `ee485cc` (implementation) plus plan-record closure;
verified by CI run `35179950199` green across all five jobs (Linux,
macOS arm64, macOS Intel, Windows incl. SCM smoke, MSRV Rust 1.89).

Depends on: the settled workspace/dependency baseline from Plan 105. This plan deliberately supersedes only Plan 105's active decision to retain Rust 1.75 and its compatibility-pin policy; Plan 105 remains the historical record of the evidence and decision that were correct at that time. This plan is a prerequisite for Plan 118.

## Objective

Raise Gregg's workspace minimum supported Rust version from 1.75 to **1.89** as an explicit product/toolchain decision, then remove the compatibility-only dependency policy that existed solely to keep fresh resolution on Rust 1.75.

The phase must leave the workspace on a clean, ordinary dependency graph suitable for the subsequent `eggfetch-core` 0.1.5 migration without changing Gregg's runtime behavior, protocol, installer ownership, service lifecycle, TUI behavior, or release architecture.

The resulting contract is:

```text
workspace MSRV: Rust 1.89
normal development toolchain: stable
prebuilt release users: no compiler requirement
Cargo/source fallback users: Rust/Cargo 1.89 or newer
```

Do not lower eggfetch's MSRV, vendor old transport dependencies, or retain obsolete 1.75-era transitive guards merely to avoid changing Gregg's declared floor.

## Baseline findings

Current `main` at the planning baseline declares `rust-version = "1.75"` in `[workspace.package]`; all four workspace members inherit it.

The ordinary CI workflow has a dedicated `MSRV (Rust 1.75)` job that installs Rust 1.75 and runs the full workspace test command. The repository otherwise develops against `stable` through `rust-toolchain.toml`.

Plan 105 audited the Rust-1.75 compatibility surface and intentionally retained 13 direct bounds/guards. The active `architecture/workspace.md` record currently describes those bounds as load-bearing for 1.75. The important distinction for this plan is that several are not Gregg-owned APIs at all: they are direct manifest entries used to constrain transitive resolution.

The client manifest currently carries the following Plan-105 compatibility block:

```text
indexmap
instability
unicode-segmentation
uuid upper bound
reqwest upper bound
url upper bound
idna
idna_adapter
hyper-rustls
quinn-proto
rustc-hash
thiserror-compat
zeroize
```

`greggd` additionally carries a compatibility-only `indexmap` bound. Some entries such as `uuid` and `url` are real source dependencies and must remain as dependencies, but their narrow upper bounds were MSRV policy rather than application API requirements. Other entries exist only to steer transitive resolution and should disappear once no longer needed.

Gregg's release/install model makes the compatibility change bounded. The five primary prebuilt targets remain binary consumers and do not need Rust installed. Source-only/fallback hosts such as Linux ARMv7 and Windows ARM64, plus direct `cargo install` users and downstream crates.io consumers, will require Rust 1.89 or newer after this phase.

## Scope decisions

### 1. Raise the workspace MSRV explicitly to Rust 1.89

Change the root workspace declaration to:

```toml
[workspace.package]
rust-version = "1.89"
```

All member crates must continue inheriting `rust-version.workspace = true`; do not duplicate the value in individual manifests.

Keep Edition 2021. Do not couple this phase to an edition migration or Cargo resolver migration.

Keep `rust-toolchain.toml` on `stable` unless a concrete local-tooling problem requires otherwise. The declared MSRV and the normal contributor toolchain serve different purposes: CI proves the floor; normal development follows stable.

### 2. Preserve Plan 105 as history; update only active policy documentation

Do not rewrite Plan 105's closure record or claim its 1.75 decision was erroneous. It was a deliberate decision under the earlier compatibility goals.

Update active documentation (`architecture/workspace.md`, current plan index, development/install docs, and any live contributor guidance) to state that Plan 117 supersedes the **current** MSRV decision and that the compatibility-pin table is historical rather than active policy.

Historical plan text and old CI run descriptions that truthfully say Rust 1.75 must remain unchanged.

### 3. Remove compatibility-only direct dependencies instead of merely widening them

After setting the floor to 1.89, perform a source-reference and resolution audit. For each Plan-105 guard, classify it as one of:

```text
REMOVE  no Gregg source imports it; it existed only to constrain a transitive
        dependency under Rust 1.75

KEEP    Gregg source imports/uses it directly; retain an ordinary direct
        dependency with the broadest semver range justified by the API

OTHER   a non-MSRV product/API compatibility reason still requires a bound;
        record that reason explicitly
```

Expected removal candidates, subject to source verification, include the transitive-only client entries:

```text
indexmap
instability
unicode-segmentation
idna
idna_adapter
hyper-rustls
quinn-proto
rustc-hash
thiserror-compat
zeroize
```

and the compatibility-only `greggd` `indexmap` entry.

Do not retain a direct dependency merely because it appears in `Cargo.lock` or in another crate's graph.

### 4. Normalize real direct dependencies that were artificially upper-bounded for Rust 1.75

`uuid` and `url` are source-level Gregg dependencies. Keep them, but remove Plan-105-only narrow upper bounds unless current source/API evidence requires a bound for another reason.

The expected shape is ordinary semver policy such as:

```toml
uuid = { version = "1", features = ["v4"] }
url = "2"
```

Exact final ranges may differ if source inspection demonstrates a concrete compatibility constraint. Record any retained nonstandard bound.

`reqwest` remains in this phase because Plan 118 owns transport replacement. Remove its Rust-1.75-only patch-level upper-bound guard and use a normal compatible 0.12 requirement for the short interval before Plan 118 removes it completely.

Do not opportunistically widen or upgrade unrelated deliberate API bounds such as Clap or Windows service-management policy unless they are proven to be part of the 1.75 compatibility block.

### 5. Re-resolve the lockfile intentionally under the new floor

The lockfile should be regenerated/updated deliberately after the compatibility-only bounds are removed. Review the resulting dependency graph rather than accepting unrelated churn blindly.

Use a clean worktree or equivalent controlled experiment to distinguish:

- required changes caused by removing the old guards;
- ordinary fresh compatible versions now allowed by Rust 1.89;
- unrelated major/API changes that should not be pulled into this phase.

The final committed `Cargo.lock` must be compatible with Rust 1.89 and with the repository's `--locked` install/release paths.

Also perform one fresh-resolution check without relying on the committed lockfile so crates.io/source consumers are not accidentally protected only by repository lock state.

### 6. Move the existing MSRV CI job; do not create another compatibility tier

Update `.github/workflows/ci.yml` in place:

```text
MSRV (Rust 1.75) -> MSRV (Rust 1.89)
toolchain: 1.75  -> toolchain: 1.89
```

Keep the existing job shape and existing native Linux/macOS/Windows jobs. Do not retain a 1.75 job in parallel and do not add a second MSRV matrix.

The MSRV job should continue running the workspace/all-targets/all-features test/check surface currently used by CI unless an exact Rust-1.89 tooling limitation requires the smallest equivalent command.

### 7. Make the source-install requirement truthful

Prebuilt binary installation behavior is unchanged. Update active installation/development documentation so direct source builds and Cargo fallback clearly require Rust 1.89+.

At minimum cover:

- `docs/installation.md` Cargo/source-only-host section;
- `docs/development.md` local build prerequisites;
- top-level and crate README text where source/Cargo installation requirements are stated;
- bootstrap installer help/fallback diagnostics where they currently tell ARMv7/source-only users only to "install Rust".

The Unix and Windows bootstrap fallbacks may state the minimum compiler version before invoking Cargo. A new custom rustc-version parser is **not required**: Cargo already enforces package `rust-version`. Prefer clear preflight/help text over a second version-comparison implementation unless current installer structure already provides a trivial safe helper.

Do not alter destination ownership, staging, checksum, service registration, restart, or update semantics under this phase.

### 8. Keep runtime/product behavior unchanged

This phase must not modify:

- wire protocol or schema versions;
- daemon collection/sampling/HTTP serving;
- client polling semantics or failure classification;
- EggPool behavior;
- TUI layout/key bindings;
- updater/download transport architecture;
- startup/service-manager ownership;
- binary target matrix or glibc floor.

Plan 118 owns the client HTTP implementation change after the new compiler/dependency baseline is settled.

## Implementation sequence

### Step 1: establish the Rust 1.89 floor

Update root `rust-version`, the existing CI MSRV label/toolchain, and active developer/install documentation that directly states the compiler floor.

Run an initial workspace check under Rust 1.89 before dependency cleanup so compiler-floor failures are separated from resolution changes.

### Step 2: audit source references for every Plan-105 compatibility entry

For each current compatibility-only manifest line, search the workspace source and record whether it is a real direct API dependency or only a resolver constraint.

Remove transitive-only entries. Normalize real direct dependencies away from 1.75-only upper bounds.

Do not infer directness solely from `cargo tree`; source imports/usages are authoritative for direct dependency ownership.

### Step 3: intentionally re-resolve dependencies

Update/regenerate `Cargo.lock` under Rust 1.89. Inspect duplicate versions and major graph changes.

Run:

```text
cargo tree --workspace -e normal
cargo tree --workspace -e normal --duplicates
```

Record any surprising new heavy dependency family rather than hiding it behind another manual transitive pin.

### Step 4: prove locked and fresh source resolution

Verify the committed lockfile through the normal local commands.

Separately, in a disposable clean worktree/copy, remove the lockfile and resolve/check the workspace under Rust 1.89. This is evidence that crates.io/source users do not depend on repository-only lock protection.

Do not commit the disposable fresh-resolution lockfile unless it matches the chosen final resolution.

### Step 5: update active architecture/install policy

Rewrite the active `architecture/workspace.md` MSRV section around Rust 1.89. Preserve a concise historical note that Plan 105 previously retained 1.75 and that Plan 117 superseded the active policy.

Update source-install/Cargo-fallback text and installer diagnostics. Do not rewrite historical plan records or old run descriptions.

### Step 6: run bounded verification

Required local checks:

```text
cargo +1.89 test --workspace --all-targets --all-features
cargo +1.89 check --workspace --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps
./scripts/check-local.sh
```

For release-facing manifest/install documentation changes, also run the existing release preflight when the tree can be cleanly exercised:

```text
./scripts/check-local.sh --release
```

Run the ordinary existing CI workflow once. The expected jobs remain Linux, macOS arm64, macOS Intel, Windows (including SCM smoke), and one Rust 1.89 MSRV job.

No new qualification workflow, artifact bundle, or extra runner is required.

## Acceptance criteria

Plan 117 is complete only when:

1. `[workspace.package].rust-version` is exactly `1.89` and all members continue to inherit it.
2. The existing CI MSRV job installs/tests Rust 1.89 and no Rust-1.75 compatibility job remains active.
3. Plan 105 remains historically intact while active architecture/docs state that Plan 117 supersedes its current MSRV decision.
4. Every Plan-105 compatibility-only direct dependency has a documented remove/keep/other disposition under the 1.89 floor.
5. Transitive-only resolver guards are removed from application manifests; genuine source dependencies remain with ordinary, justified semver requirements.
6. `Cargo.lock` is intentionally re-resolved and all `--locked` repository/install paths remain valid.
7. A disposable no-lock fresh resolution checks successfully under Rust 1.89.
8. Direct Cargo/source installs and source-only bootstrap fallback documentation truthfully require Rust 1.89+; prebuilt binary users remain unaffected.
9. No runtime/protocol/TUI/service/update behavior changes are introduced.
10. The default local check, applicable release preflight, and existing native CI workflow pass on the final tree.
11. The plan closure records the implementation SHA, final dependency decisions, and exact CI run used for native/MSRV evidence.

## Preserved exclusions

- no Edition 2024 migration;
- no Cargo resolver migration solely because the compiler floor changed;
- no `eggfetch` integration yet (Plan 118);
- no new HTTP/TLS behavior;
- no removal of real direct dependencies merely to reduce line count;
- no speculative upgrades of unrelated API-bounded dependencies;
- no new CI workflow/job family/matrix;
- no binary target or glibc-floor changes;
- no installer ownership/lifecycle redesign;
- no rewriting of Plan 105 or other truthful historical closure records.

## Closure record

Implementation `ee485cc` ("chore: raise MSRV to Rust 1.89 and retire 1.75
resolver pins (Plan 117)"), verified by remote CI run `35179950199`
(all five jobs green: Linux, macOS arm64, macOS Intel, Windows incl. SCM
smoke, MSRV Rust 1.89). Local evidence on the implementation tree:
`cargo +1.89 check` before and after dependency cleanup,
`cargo +1.89 test --workspace --all-targets --all-features` (one
environmental `gregg-update` curl-classification flake, green on rerun),
stable `cargo test --workspace --all-targets --all-features` (1109
passed, 0 failed), stable
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean, `cargo fmt --all -- --check` clean,
`cargo doc --workspace --no-deps` (only pre-existing warnings, identical
at base), `./scripts/check-local.sh` (default) and
`./scripts/check-local.sh --release` both pass.

Final dependency decisions under the 1.89 floor (source imports
authoritative; `cargo tree` never retained a direct entry):

```text
REMOVE  gregg indexmap, greggd indexmap (transitive via toml_edit 2.14.2)
REMOVE  instability (transitive via ratatui 0.3.13)
REMOVE  unicode-segmentation (transitive via ratatui 1.13.3)
REMOVE  idna / idna_adapter (transitive via url 2.5.8)
REMOVE  hyper-rustls 0.27.9 (transitive via reqwest)
REMOVE  quinn-proto 0.11.18 (transitive via reqwest HTTP/3)
REMOVE  rustc-hash (transitive)
REMOVE  zeroize 1.9.0 (transitive)
REMOVE  thiserror-compat (transitive thiserror 2.x guard)
KEEP    uuid = { version = "1", features = ["v4"] } (Uuid::new_v4)
KEEP    url = "2" (eggpool.rs Url)
KEEP    reqwest = { version = "0.12", ... } (Systems poller, EggPool,
        endpoint URL adapter; resolved 0.12.28; Plan 118 owns removal)
OTHER   none. Unrelated Clap / windows-service bounds untouched.
```

`cargo update` under the new floor produced only compatible minor/patch
movement (url 2.5.4→2.5.8, uuid 1.20.0→1.26.1, quinn-proto→0.11.18,
hyper-rustls→0.27.9, zeroize→1.9.0, unicode-segmentation→1.13.3) plus the
icu chain via idna 1.1; remaining `--duplicates` (getrandom, hashbrown,
heck, rustix, strsim, syn v2/v3, unicode-width) are ordinary unrelated
family divergences, not new weight. A disposable no-lock fresh
resolution under Rust 1.89 checked clean and produced a byte-identical
lockfile (name/version set), so crates.io consumers share the committed
resolution.

Incidental corrective (required for green CI, enabled by the new floor):
four pre-existing patterns newly flagged by stable clippy 1.98 were
modernized with zero behavior change — `map_or(true, …)` →
`is_none_or(…)` in `greggd` control/server/uninstall (1.82+ API),
`count % 2 == 0` → `count.is_multiple_of(2)` in `gregg` scheduler test
helper (1.87+ API). Both APIs predate the 1.89 floor. The pre-existing
1.89-clippy `const_is_empty` warning in Windows-only service test code
is warn-level, untouched, and CI-safe (MSRV/Windows jobs do not run
clippy).

Plan 105's historical record is unchanged; active policy
(`architecture/workspace.md`, `AGENTS.md`, install/dev docs, installer
diagnostics, crate READMEs, `CHANGELOG.md`) now states Rust 1.89+.
`.opencode/skills/` carries no MSRV policy and needed no change.
