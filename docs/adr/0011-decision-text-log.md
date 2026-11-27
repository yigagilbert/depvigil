# 0011. Mirror decisions in a text log

- **Status:** Proposed
- **Date:** 2026-11-27

## Context

A Relate store is binary, so a committed store shows up as an unreadable diff in a pull request.

## Decision

Each time a `Decision` or `Invalidation` is written, one line is appended to `.depvigil/decisions.jsonl`. The store remains the source of truth.

## Consequences

- Reviewers can read what was decided in a pull request.
- A test checks that the text log and the store agree.

## Evidence

Not yet tested.
