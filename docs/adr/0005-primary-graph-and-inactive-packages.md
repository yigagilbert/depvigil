# 0005. Use the cargo metadata resolve graph as the primary graph and keep lockfile-only packages as inactive

- **Status:** Proposed
- **Date:** 2026-11-27

## Context

`Cargo.lock` is a superset of the resolve graph: for `bat` it holds nine packages that no enabled feature uses. `cargo audit` reads the lockfile, so those packages must stay visible, but they are not part of what is built.

## Decision

The resolve graph from `cargo metadata`, for all platforms, is the primary dependency graph. Packages that appear only in `Cargo.lock` are stored as well and flagged inactive on the snapshot's `INCLUDES` edge.

## Consequences

- A finding on an inactive package is reported as a warning, not a rejection.
- Activity is known only for snapshots resolved with `cargo metadata`. A snapshot built from the lockfile alone records no activity.

## Evidence

Phase 0 Tests 2 and 3. Storing and flagging inactive packages is not yet tested.
