# 0008. Detect and report unsupported inputs

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

A manifest-only copy cannot see a path dependency outside the copied tree. Git dependencies and `[patch]` sections were not tested in Phase 0.

## Decision

The following are detected and reported as unsupported. They are never resolved silently:

- git dependencies;
- `[patch]` sections;
- path dependencies outside the workspace root.

## Consequences

- Commands that would simulate or snapshot such a workspace stop with a non-zero exit code and name the input.

## Evidence

`tests/ecosystem.rs` and `tests/check.rs`: each of the three inputs is detected, named and refused with exit code 3 before any Cargo command runs.
