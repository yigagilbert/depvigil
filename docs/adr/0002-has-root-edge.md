# 0002. Mark workspace members with a HAS_ROOT edge

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

Explaining a finding needs a dependency path from the project root to the affected version. The draft schema could not say which versions are the root from the graph alone.

## Decision

Every snapshot has a `(Snapshot)-[:HAS_ROOT]->(Version)` edge to each workspace member.

## Consequences

- Path queries start from `HAS_ROOT` and need no root passed in from outside.
- A workspace with several members has several roots; the shortest path over all of them is reported.

## Evidence

Phase 0 Test 4.
