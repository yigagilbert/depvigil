# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Core engine: `init`, `advisories load`, `check`, `verify`, `recheck`, `log`
  and `why`, each with `--json`.
- Cargo support behind an ecosystem-neutral `Ecosystem` trait.
- A Relate directory store at `.depvigil/store` and a text mirror of
  decisions at `.depvigil/decisions.jsonl`.
- Architecture decision records under `docs/adr/`.

### Changed

- The Phase 0 feasibility crate was replaced by the `depvigil` crate. The
  Phase 0 report moved to `docs/feasibility/phase0.md`.
