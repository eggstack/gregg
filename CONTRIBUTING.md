# Contributing to gregg

Thank you for considering a contribution to gregg.

## Getting started

1. Fork the repository and create a feature branch.
2. Ensure the pinned toolchain is installed: `rustup show`.
3. Run the full check suite before submitting:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

4. Open a pull request against `main`.

## Project scope

gregg is intentionally narrow. It is a compact terminal monitor for
system-level metrics across multiple machines. Before proposing a change,
verify it fits within the goals documented in `README.md` and the active
phase plan in `plans/`.

Scope-expanding changes should be recorded as post-version-1 ideas unless
they are necessary for correctness, safety, publishability, or the explicit
product contract.

## Code conventions

- Follow existing code style and patterns.
- Platform-specific code lives under `cfg(target_os = ...)` modules.
- Unsafe Rust is permitted only in the macOS collector FFI module
  (`crates/greggd/src/collector/macos/ffi.rs`).
- Dependencies must solve a concrete version-1 requirement. Avoid adding
  dependencies without discussion.
- Configuration writes must be atomic (write-flush-rename-verify).
- Human-readable output goes to stdout; diagnostics go to stderr.
- Tests must not sleep for production refresh intervals.

## Commit messages

Keep commits scoped to one phase or one coherent corrective pass. Update
documentation and tests with behavioral changes.

## License

By contributing, you agree that your contributions will be licensed under
the MIT License.
