# Gregg version-1 plan index

This directory contains the implementation roadmap and execution-ready plans for taking Gregg through a credible, crates.io-published version 1.

The plans are ordered by dependency. A later phase may begin early only where its interfaces are already frozen and doing so does not bypass an earlier acceptance gate.

`1.0.0` was published after the initial implementation attempt, but subsequent review found unresolved native macOS, daemon supervision, service lifecycle, client configuration, packaging, and release-evidence defects. Phase 11 is the active source of truth for corrective closure and a `1.0.1` patch release.

| Plan | Purpose | Primary output | Status |
| --- | --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Program-level architecture, sequencing, risks, and release definition | Version-1 execution map | active |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | Publishable protocol crate and stable contracts | implemented |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | Tested Linux collector | implemented; verification carried into phase 11 |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | Tested macOS collector | reopened by phase 11 |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | Functional foreground daemon | reopened by phase 11 |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | Deployable `greggd` | reopened by phase 11 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and `add/list/remove/refresh/edit` commands | Scriptable client configuration | reopened by phase 11 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | Maintainable non-visual client core | reopened by phase 11 |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, adaptive width, navigation, scrolling | Complete compact TUI | implemented; native/manual verification remains in phase 11 |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Cross-platform failures, soak tests, resource budgets, packaging validation | Release-candidate evidence | reopened by phase 11 |
| [`010-cratesio-release-v1.md`](010-cratesio-release-v1.md) | Documentation closure, package verification, publication and tagging | crates.io version 1.0.0 release | `1.0.0` published; closure criteria incomplete |
| [`011-v1.0.1-corrective-closure.md`](011-v1.0.1-corrective-closure.md) | Correct native, runtime, lifecycle, client, packaging, and evidence defects | Verified `1.0.1` corrective release | active |

## Completion rule

A plan is not complete merely because its implementation has landed. It is complete only when all acceptance criteria are demonstrated by tests, CI, reproducible commands, or documented manual evidence appropriate to the target platform.

A plan may be marked `implemented` only after every acceptance criterion is satisfied with evidence in the tree or an immutable linked CI/release record. Mock coverage cannot substitute for required native Linux/macOS or service-manager evidence.

Any discovered scope expansion should be recorded as a post-version-1 idea unless it is necessary for correctness, safety, publishability, or the explicit product contract in `README.md`.