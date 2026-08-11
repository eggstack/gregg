---
name: rust-workspace
description: Build, test, and verify the gregg Rust workspace
---

## What I do

Guide agents through building, testing, and verifying changes to the gregg workspace.

## When to use me

Use this when making code changes, adding features, or fixing bugs in any of the three crates.

## Build and verify

**Routine local check (fast developer loop):**

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

Runs `cargo fmt --all -- --check` followed by `cargo test --workspace`. Does not repeat native collector tests, build docs, or run release checks.

**Release preflight (non-publishing, manual only):**

```bash
./scripts/check-local.sh --release
```

Adds: Clippy, documentation, clean-tree check, version consistency, package lists, installed-binary v2 loopback smoke, and protocol dry-run.

**Platform-native collector tests (run separately when focused coverage is needed):**

```bash
cargo test -p greggd --all-features -- collector::linux     # Linux
cargo test -p greggd --all-features -- collector::macos     # macOS
cargo test -p greggd --all-targets -- collector::windows    # Windows
```

**Running a single test:**

```bash
cargo test -p gregg-protocol -- <test_name>
cargo test -p greggd --all-features -- <test_name>
```

For Systems endpoint-reload changes, include the production-path bounded
command-pressure and sequential replacement tests in `cargo test -p gregg
--bin gregg`. A successful reload must await the existing bounded scheduler
sender; do not make the channel unbounded or discard replacement-send errors.

**CI note:** GitHub Actions sets `RUSTFLAGS: -D warnings`, making all warnings errors. Local clippy pedantic is a warning only.

For workspace structure, key constraints, and what not to do, see `AGENTS.md`. For deep architecture details, see `architecture/overview.md`.
