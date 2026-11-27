# 0003. Bind approvals with a digest over packages, edges and declared dependencies

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

Phase 0 Test 5 bound an approval to the package list only. Adding the approved crate to a different workspace member then produced the same digest. The digest must also be computable without `cargo metadata`, because a clean candidate is approved from the lockfile alone (ADR 0006).

## Decision

The digest is a SHA-256 over a canonical text built from data that both simulation steps have:

1. every lockfile package: name, version, source, checksum;
2. every lockfile dependency edge;
3. for each workspace member, every declared dependency with its requirement, features, `default-features`, `optional`, kind (normal, dev, build), target condition and rename, and the member's own `[features]` table;
4. the workspace resolver version.

Path dependencies appear as `path:<manifest directory relative to the workspace root>`. No absolute path enters the digest.

## Consequences

- Features are bound as declared in the manifests, not as resolved by Cargo. Two states with the same lockfile and the same declarations have the same resolved features, so this is equivalent for a given Cargo version.
- `verify` needs no `cargo metadata` run and no network.
- The canonical text is stored with the snapshot, so a mismatch can name the lines that differ.

## Evidence

`tests/verify.rs`: exact application and a checkout in another directory match; a different version, an extra crate, the approved crate on the wrong member and a changed feature set all mismatch and are named. `tests/ecosystem.rs`: one digest for the lockfile and the full view, with no absolute path in the canonical text.
