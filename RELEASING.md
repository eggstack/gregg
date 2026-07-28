# Releasing gregg

Releases are performed manually. GitHub Actions verifies source changes and
does not publish artifacts or releases.

## Quick reference

1. Ensure all local checks pass:

   ```text
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-targets --all-features
   cargo doc --workspace --no-deps
   cargo deny check
   ```

2. Verify packages:

   ```text
   cargo package -p gregg-protocol --list
   cargo package -p greggd --list
   cargo package -p gregg --list
   ```

3. Publish in order: `gregg-protocol`, then `greggd`, then `gregg`.

4. Create an annotated Git tag and a GitHub Release manually.

This document is a placeholder. The full operator runbook will be written in
Phase 39 (`plans/039-manual-cratesio-and-github-release.md`).
