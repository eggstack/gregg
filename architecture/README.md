# Architecture

Documents in this directory describe how `gregg` is structured and why.
Implementation details that change with the code live in rustdoc and inline
comments; this directory captures decisions that are larger than a single crate
and that contributors should read before reorganising code across boundaries.

## Overview and deep dives

| Document | Purpose |
| --- | --- |
| [`overview.md`](overview.md) | Bird's-eye view of the entire codebase: data flow, module map, index of all documents. |
| [`gregg-protocol.md`](gregg-protocol.md) | Deep dive: protocol crate — wire types, schema versions, validation, test support. |
| [`greggd-daemon.md`](greggd-daemon.md) | Deep dive: daemon crate — collectors, sampler, HTTP server, service management. |
| [`gregg-client.md`](gregg-client.md) | Deep dive: client crate — CLI, polling, state engine, TUI, EggPool. |
| [`collectors.md`](collectors.md) | Deep dive: platform collectors — Linux, macOS, Windows native metric collection. |
| [`scripts-and-packaging.md`](scripts-and-packaging.md) | Deep dive: scripts, installers, service definitions, CI. |

## Cross-cutting decisions

| Document | Purpose |
| --- | --- |
| [`workspace.md`](workspace.md) | Cargo workspace layout, member responsibilities, dependency direction, and crate-boundary rules. |
| [`protocol.md`](protocol.md) | Schema-version wire contract, capabilities, validation, compatibility policy, and collector contract. |
| [`error-conventions.md`](error-conventions.md) | Typed error boundaries, command-level diagnostics, and what may or may not appear in wire responses. |
| [`macos-collector-notes.md`](macos-collector-notes.md) | Expected differences between the macOS collector and Activity Monitor / `top` / `vm_stat`. |

Phase plans under [`plans/`](../plans/) are the source of truth for sequencing
and acceptance criteria; this directory mirrors the architectural commitments
that several phases must respect together.
