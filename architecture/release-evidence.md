# Release evidence architecture

Release evidence has three boundaries. Current-run dispatch output is a
diagnostic summary and may be created before GitHub assigns artifact IDs.
Cross-run pre-tag and final manifests are immutable selections: each logical
stage binds to one retrieved GitHub artifact and records its numeric ID, exact
name, downloaded ZIP SHA-256, and size.

The finalizer checks out the frozen candidate SHA for executable tooling. Run
selection and the operator-authorized historical disposition created after that
freeze are data supplied through bounded base64 workflow inputs. Each decoded
UTF-8 JSON document is validated before network access, written atomically,
hashed, and included with workflow run and actor identity. The selection source
remains `workflow-dispatch-base64` through retrieval and aggregation.

Package provenance declares exact relative archive and lockfile paths. The
retrieval index resolves only those declarations, verifies containment, digest,
size, package name, and version, and rejects ambiguity. Boundary 2 reuses the
selected Boundary-1 archive bytes, semantically compares the Cargo.lock
protocol checksum with the validated registry response, and retains locked
build, test, install, help/version, and tool-version command transcripts.

The sustained workload retains the scheduler task handle. Cancellation is
clean only after a bounded join returns success; channel closure alone cannot
qualify shutdown, and timeout cleanup aborts and awaits the task.
