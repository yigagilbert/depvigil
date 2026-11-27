# 0001. Store the graph in a Relate directory store

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

The CLI and an agent server must be able to run at the same time. Phase 0 Test 1 showed that a `.rl` file is locked whole by its writer: a second process can neither write nor read it. A directory store still has one writer, but other processes can open it read-only or follow its commits.

## Decision

The graph lives in a Relate directory store at `.depvigil/store` inside the project, never in a single `.rl` file.

## Consequences

- Only one process writes at a time. A second writer gets a `storage locked` error and must retry or read only.
- Read-only commands can run beside a writer.
- A directory store can still be exported as one file with Relate's clone operation.

## Evidence

Phase 0 Test 1; `tests/store.rs`.
