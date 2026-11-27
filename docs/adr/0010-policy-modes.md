# 0010. Reject only vulnerabilities a candidate introduces, with a strict mode

- **Status:** Proposed
- **Date:** 2026-11-27

## Context

A workspace may already contain known vulnerabilities. Rejecting every candidate until they are fixed would make the tool useless on such a workspace.

## Decision

`depvigil.toml` selects a policy mode:

- `introduced` (the default): a candidate is rejected only when it introduces a vulnerable version the baseline lacks. Existing vulnerabilities are reported as context, and a vulnerability the candidate removes is reported as an improvement.
- `strict`: a candidate is rejected when its dependency set contains any vulnerability.

Notices warn in both modes.

## Consequences

- A project can adopt the tool before it has cleared its existing findings.

## Evidence

Not yet tested.
