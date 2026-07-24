# Gregg version-1 plan index

This directory contains the implementation roadmap and execution-ready plans for taking Gregg through a credible, crates.io-published version 1.

The plans are ordered by dependency. A later phase may begin early only where its interfaces are already frozen and doing so does not bypass an earlier acceptance gate.

`1.0.0` was published after the initial implementation attempt. Phase 11 corrected a meaningful subset of the discovered defects, including the Darwin swap ABI layout, daemon pre-bind behavior, stale-snapshot policy wiring, explicit-port parsing, immediate polling, bounded streaming foundations, batch preservation after task failure, and CI expansion. A follow-up review found remaining native Mach ownership, client persistence/locking, scheduler, strict response-bound, daemon supervision, launchd, installer, systemd privilege, architecture, package, and release-evidence gaps. Phase 12 partially implemented the corrective closure; Phase 13 is the active source of truth for final correctness fixes, release-candidate evidence, and the `1.0.1` patch release.

| Plan | Purpose | Primary output | Status |
| --- | --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Program-level architecture, sequencing, risks, and release definition | Version-1 execution map | active |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | Publishable protocol crate and stable contracts | implemented |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | Tested Linux collector | implemented; final native evidence carried into phase 12 |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | Tested macOS collector | implemented; final ownership and dual-architecture evidence in phase 12 |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | Functional foreground daemon | implemented; final supervision closure in phase 12 |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | Deployable `greggd` | implemented; lifecycle and installer closure in phase 12 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and `add/list/remove/refresh/edit` commands | Scriptable client configuration | implemented; persistence and locking closure in phase 12 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | Maintainable non-visual client core | implemented; scheduler closure in phase 12 |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, adaptive width, navigation, scrolling | Complete compact TUI | implemented; native/manual verification remains in phase 12 |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Cross-platform failures, soak tests, resource budgets, packaging validation | Release-candidate evidence | implemented; measured evidence in phase 12 |
| [`010-cratesio-release-v1.md`](010-cratesio-release-v1.md) | Documentation closure, package verification, publication and tagging | crates.io version 1.0.0 release | `1.0.0` published; original closure criteria incomplete |
| [`011-v1.0.1-corrective-closure.md`](011-v1.0.1-corrective-closure.md) | First correction of native, runtime, client, CI, and documentation defects | Initial `1.0.1` corrective implementation | implemented; unresolved items superseded by phase 12 |
| [`012-v1.0.1-final-corrective-closure.md`](012-v1.0.1-final-corrective-closure.md) | Close remaining native, concurrency, scheduler, lifecycle, packaging, evidence, and release defects | Verified final `1.0.1` corrective release | partially implemented; remaining closure superseded by phase 13 |
| [`013-v1.0.1-release-gate-closure.md`](013-v1.0.1-release-gate-closure.md) | Close remaining correctness defects and release-evidence gaps for a verifiable `1.0.1` release | Release-candidate evidence, tagged `1.0.1` | active |

## Completion rule

A plan is not complete merely because its implementation has landed. It is complete only when all acceptance criteria are demonstrated by tests, CI, reproducible commands, or documented manual evidence appropriate to the target platform.

A plan may be marked `implemented` only after every acceptance criterion is satisfied with evidence in the tree or an immutable linked CI/release record. Mock coverage cannot substitute for required native Linux/macOS or service-manager evidence.

Phase 13 may be marked implemented only after the published `1.0.1` source, annotated tag, package checksums, native-platform runs, service lifecycle results, clean package installs, dependency disposition, and measured resource/soak evidence are all reconciled in `plans/v1.0.1-final-evidence.md`.

Any discovered scope expansion should be recorded as a post-version-1 idea unless it is necessary for correctness, safety, publishability, or the explicit product contract in `README.md`.