---
name: architecture-docs
description: Read and update architecture documentation in the architecture/ directory
---

## What I do

Guide agents through the architecture documentation structure and how to keep it accurate.

## When to use me

Use this when implementing features that change crate boundaries, module structure, data flow, or cross-cutting concerns. Read relevant architecture docs before making structural changes.

## Architecture document index

| Document | Scope |
|----------|-------|
| [`overview.md`](../../architecture/overview.md) | Bird's-eye view: data flow, module map, index of all documents |
| [`gregg-protocol.md`](../../architecture/gregg-protocol.md) | Deep dive: protocol crate — wire types, schema versions, validation, test support |
| [`greggd-daemon.md`](../../architecture/greggd-daemon.md) | Deep dive: daemon crate — collectors, sampler, HTTP server, service management |
| [`gregg-client.md`](../../architecture/gregg-client.md) | Deep dive: client crate — CLI, polling, state engine, TUI, EggPool |
| [`collectors.md`](../../architecture/collectors.md) | Deep dive: platform collectors — Linux, macOS, Windows native metric collection |
| [`scripts-and-packaging.md`](../../architecture/scripts-and-packaging.md) | Deep dive: scripts, installers, service definitions, CI |
| [`workspace.md`](../../architecture/workspace.md) | Cargo workspace layout, member responsibilities, dependency direction, crate-boundary rules |
| [`protocol.md`](../../architecture/protocol.md) | Schema-version wire contract, capabilities, validation, compatibility policy |
| [`error-conventions.md`](../../architecture/error-conventions.md) | Typed error boundaries, command-level diagnostics, wire response constraints |
| [`macos-collector-notes.md`](../../architecture/macos-collector-notes.md) | Expected differences between macOS collector and Activity Monitor / top / vm_stat |

## When to update architecture docs

Update architecture docs when:
- Adding or removing a module that changes the module map
- Changing data flow between components
- Modifying the SystemCollector trait or collector contract
- Adding new wire types or changing validation rules
- Changing configuration format or paths
- Adding new CLI subcommands
- Changing platform-specific behavior

## How to update

1. Read the relevant architecture document
2. Update the section that changed
3. Update the module map tables if modules were added/removed/renamed
4. Update the data flow diagram if component interactions changed
5. Keep the document index in `overview.md` accurate

## Relationship to plans

Phase plans under `plans/` are the source of truth for sequencing and acceptance criteria. Architecture documents capture decisions that multiple phases must respect together.
