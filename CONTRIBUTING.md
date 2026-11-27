# Contributing

Thanks for your interest in depvigil.

## Getting started

```bash
git clone https://github.com/yigagilbert/depvigil
cd depvigil
cargo test
```

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first
use. The tests simulate dependency changes with `cargo add`, so they need
network access to the crates.io index.

Two fixtures are git submodules. Tests that need them skip with a message when
they are absent. To run everything:

```bash
git submodule update --init
cargo install cargo-audit --locked
cargo test
```

## Before you open a pull request

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same three commands.

## Ground rules

- **Never build a candidate.** Code and tests must not run `cargo build`,
  `cargo check`, `cargo test` or `cargo run` against a candidate dependency
  set, and must not use `--filter-platform` or `cargo generate-lockfile` for
  simulation. See `docs/adr/0006-two-step-simulation.md`.
- **Fixtures are read-only.** Work on a temporary copy.
- **No string-built Cypher.** Every query is a constant; values travel as
  parameters.
- **Typed errors.** Library code returns `depvigil::Error` and does not panic
  on bad input. Public items have doc comments.
- **Tests come with the feature.** Do not weaken a test to make it pass.
- **Decisions are recorded.** A change of design gets a record in `docs/adr/`.

## Dependencies

This project checks its own dependencies with depvigil. Before adding one,
run `depvigil check <crate>` and commit the updated `.depvigil` directory with
the change.

## Licence

Contributions are accepted under the project's dual licence, MIT or
Apache-2.0, at the user's option.
