# Release evidence architecture

Release evidence has three boundaries. Current-run dispatch output is a
diagnostic summary and may be created before GitHub assigns artifact IDs.
Cross-run pre-tag and final manifests are immutable selections: each logical
stage binds to one retrieved GitHub artifact and records its numeric ID, exact
name, downloaded ZIP SHA-256, and size.

The finalizer checks out the frozen candidate SHA for executable tooling. Run
selection created after that freeze is data supplied through the bounded
`selection_base64` workflow input. The decoded UTF-8 JSON is validated before
network access, written atomically, hashed, and included in the manifest with
workflow run identity.

Package provenance declares exact relative archive and lockfile paths. The
retrieval index resolves only those declarations, verifies containment, digest,
size, package name, and version, and rejects ambiguity. Boundary 2 reuses the
selected Boundary-1 archive bytes and performs a fresh crates.io registry
resolution, locked build, tests, clean install, and binary help/version smoke.

The sustained workload retains the scheduler task handle. Cancellation is
clean only after a bounded join returns success; channel closure alone cannot
qualify shutdown, and timeout cleanup aborts and awaits the task.
