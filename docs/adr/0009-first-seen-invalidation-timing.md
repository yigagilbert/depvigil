# 0009. Time invalidations by when an advisory first affected a version

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

Comparing an advisory's publication date with the decision date misses real cases: an advisory published three days ago but loaded today would not invalidate yesterday's approval. An advisory edited later to cover more versions must also be able to invalidate decisions it did not affect before.

## Decision

- Each `AFFECTS` relationship (an advisory and version pair) records when the store first saw it, as `first_seen_at`.
- An approval is invalidated when an `AFFECTS` relationship to a version in its snapshot was first seen after the decision.
- Every advisory load is recorded as an `AdvisoryLoad` node with its source, commit and load time. Each `Decision` links to the load it was judged against.
- The advisory's publication date is still shown.

## Consequences

- Every approval states what evidence it had.
- The rule depends on the store's own clock, not on dates supplied by the advisory.

## Evidence

`tests/advisories.rs`: first-seen times are set once and never moved, and an edited advisory gains and loses versions. `tests/recheck.rs`: the rule invalidates on first-seen time while showing the publication date, and each decision links to its advisory load.
