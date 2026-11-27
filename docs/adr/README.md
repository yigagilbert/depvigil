# Architecture decision records

One file per decision. A record is `Proposed` until a test covers it, then `Accepted`.

| # | Decision | Status |
| --- | --- | --- |
| [0001](0001-directory-store.md) | Store the graph in a Relate directory store | Accepted |
| [0002](0002-has-root-edge.md) | Mark workspace members with a HAS_ROOT edge | Accepted |
| [0003](0003-approval-digest.md) | Bind approvals with a digest over packages, edges and declared dependencies | Proposed |
| [0004](0004-invalidation-per-pair-and-advisory-kind.md) | Record one invalidation per decision and advisory, by advisory kind | Proposed |
| [0005](0005-primary-graph-and-inactive-packages.md) | Use the cargo metadata resolve graph as the primary graph and keep lockfile-only packages as inactive | Proposed |
| [0006](0006-two-step-simulation.md) | Simulate a change in two steps | Proposed |
| [0007](0007-osv-advisory-source.md) | Use the OSV export as the only advisory source | Accepted |
| [0008](0008-unsupported-inputs.md) | Detect and report unsupported inputs | Proposed |
| [0009](0009-first-seen-invalidation-timing.md) | Time invalidations by when an advisory first affected a version | Proposed |
| [0010](0010-policy-modes.md) | Reject only vulnerabilities a candidate introduces, with a strict mode | Proposed |
| [0011](0011-decision-text-log.md) | Mirror decisions in a text log | Proposed |
