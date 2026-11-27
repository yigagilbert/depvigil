# 0004. Record one invalidation per decision and advisory, by advisory kind

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

The Phase 0 query skipped a decision once it had any invalidation, so a second advisory against the same approval would go unrecorded. It also treated an informational notice like a vulnerability.

## Decision

- An `Invalidation` is recorded once for each (decision, advisory) pair.
- Every `Advisory` has a kind: `vulnerability` or `notice`.
- A vulnerability invalidates an approval. A notice produces a warning and does not invalidate.
- A `Decision` node is never modified. Its current status is derived from its invalidations.
- A decision whose every invalidating advisory has been withdrawn is reported as restored.

## Consequences

- Running `recheck` twice creates nothing new.
- A restored decision keeps its invalidation records as history.
- An approval made by the fast step records no activity, so a later vulnerability in its lockfile invalidates it even when the package may be inactive.

## Evidence

`tests/recheck.rs`: one invalidation per pair, a second advisory gets its own, notices only warn, `Decision` nodes are unchanged, a second run adds nothing, a withdrawn advisory restores the decision. `tests/advisories.rs`: every loaded advisory has a kind.
