# Gregg version-1 plan index

This directory contains the implementation roadmap and execution-ready plans for taking the fresh repository through a crates.io-published version 1.

The plans are ordered by dependency. A later phase may begin early only where its interfaces are already frozen and doing so does not bypass an earlier acceptance gate.

| Plan | Purpose | Primary output | Status |
| --- | --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Program-level architecture, sequencing, risks, and release definition | Version-1 execution map | active |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | Publishable protocol crate and stable contracts | implemented |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | Tested Linux collector | not started |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | Tested macOS collector | not started |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | Functional foreground daemon | not started |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | Deployable `greggd` | not started |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and `add/list/remove/refresh/edit` commands | Scriptable client configuration | not started |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | Maintainable non-visual client core | not started |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, adaptive width, navigation, scrolling | Complete compact TUI | not started |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Cross-platform failures, soak tests, resource budgets, packaging validation | Release-candidate evidence | not started |
| [`010-cratesio-release-v1.md`](010-cratesio-release-v1.md) | Documentation closure, package verification, publication and tagging | crates.io version 1.0.0 release | not started |

## Completion rule

A plan is not complete merely because its implementation has landed. It is complete only when all acceptance criteria are demonstrated by tests, CI, reproducible commands, or documented manual evidence appropriate to the target platform.

A plan is marked `implemented` in the table above only after every acceptance
criterion in that plan is satisfied with evidence in the tree (tests, CI, or
documented manual runs).

Any discovered scope expansion should be recorded as a post-version-1 idea unless it is necessary for correctness, safety, publishability, or the explicit product contract in `README.md`.
