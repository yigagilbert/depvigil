# 0007. Use the OSV export as the only advisory source

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

The RustSec OSV export carries the `published`, `modified` and `withdrawn` dates that Living Approvals needs. In Phase 0 the project's own range evaluator agreed with `cargo audit` on every finding in both fixtures.

## Decision

The RustSec OSV export is the only advisory source in the product. `cargo audit` is ground truth in tests and nothing more.

## Consequences

- The product does not depend on `cargo-audit` or on the advisory database's own format at run time.
- Only `SEMVER` ranges with `introduced` and `fixed` events are evaluated. Anything else is rejected, not guessed.

## Evidence

Phase 0 Test 4, Part A; `tests/advisories.rs` (findings equal `cargo audit` on both fixtures).
