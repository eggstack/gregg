# Workspace and crate boundaries

The repository is a Cargo workspace with three independently publishable
members under `crates/`:

```text
crates/gregg-protocol    library   versioned wire types and compatibility rules
crates/greggd           binary    Linux/macOS metrics daemon + service-management CLI
crates/gregg            binary    endpoint-management CLI + polling/state engine + Ratatui TUI
```

## Dependency direction

```text
gregg-protocol  ◄── greggd
gregg-protocol  ◄── gregg
```

Allowed:

- `gregg-protocol` depends only on narrow serialization and error crates.
- `greggd` and `gregg` may each depend on `gregg-protocol`.

Forbidden:

- `gregg-protocol` depending on either binary crate.
- `greggd` depending on `gregg`, or vice versa.
- Sharing implementation code through `gregg-protocol` to avoid creating a new
  internal module in the consuming crate.

## Internal module boundaries

Within each binary crate, the following are kept separate:

- Native collection is distinct from sampling and HTTP serving.
- Service management is distinct from the foreground daemon process.
- Client polling is distinct from application-state reduction.
- The renderer reads state; it does not perform I/O or mutate polling internals.
- Platform-specific code remains under narrow `cfg(target_os = ...)` modules.

## MSRV

The workspace declares `rust-version = "1.75"` in `[workspace.package]` and
inherits it in every member manifest. Nightly-only language or Cargo features
must not be used. The Rust toolchain pinned in `rust-toolchain.toml` is the
current stable release; CI installs the same channel so formatting and lint
behaviour stay aligned with local development.

## Lints

The workspace enables `clippy::pedantic` as a warning (not an error) so that
contributors see style suggestions without breaking the build on unrelated
changes. The two binary crates and `gregg-protocol` all `#[deny(unsafe_code)]`
through the workspace lint table; macOS collector FFI is the only planned
exception and will be scoped to one module in a later phase.
