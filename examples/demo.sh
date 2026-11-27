#!/usr/bin/env bash
#
# depvigil end-to-end demo on the `small` fixture.
#
# It works on a temporary copy of the fixture and never builds anything.
# Run it from anywhere:
#
#     examples/demo.sh
#
# Environment:
#   DEPVIGIL   path to a built depvigil binary (default: build one with cargo)
#
# Network: `depvigil init` and a rejected candidate run `cargo metadata`,
# which downloads the crates in the dependency graph when the Cargo cache is
# cold. An approved candidate needs crates.io index data only.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADVISORIES="$ROOT/data/advisories/rustsec-osv.jsonl.gz"
# The day the advisory against the fixture's `regex` version was published.
CUTOFF="2022-03-08"

if [ -z "${DEPVIGIL:-}" ]; then
    (cd "$ROOT" && cargo build --quiet)
    DEPVIGIL="$ROOT/target/debug/depvigil"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
PROJECT="$WORK/project"
cp -R "$ROOT/fixtures/small" "$PROJECT"

step() {
    printf '\n== %s\n' "$1"
}

# Runs depvigil and checks that it exits with the expected code.
expect() {
    local expected="$1"
    shift
    local actual=0
    "$DEPVIGIL" --project "$PROJECT" "$@" || actual=$?
    if [ "$actual" -ne "$expected" ]; then
        echo "demo: \`depvigil $*\` exited with $actual, expected $expected" >&2
        exit 1
    fi
}

step "1. Record the workspace as the baseline"
expect 0 init

step "2. Load the advisories that were known before $CUTOFF"
echo "(This demo simulates the passage of time: it first loads only the"
echo " advisories published before $CUTOFF, and the rest in step 7.)"
expect 0 advisories load "$ADVISORIES" --published-before "$CUTOFF"

step "3. Check a bad candidate: chrono 0.4.19 has a known vulnerability"
expect 1 check chrono@=0.4.19 --package small-app

step "4. Check a good candidate: either 1.13.0"
expect 0 check either@=1.13.0 --package small-core

step "5. Apply the approved change to the workspace"
(cd "$PROJECT" && cargo add either@=1.13.0 --package small-core --quiet)
echo "ran: cargo add either@=1.13.0 --package small-core"

step "6. Verify that the workspace is exactly what was approved"
expect 0 verify

step "7. Time passes: the advisory database is updated"
expect 0 advisories load "$ADVISORIES"

step "8. Recheck past approvals against what is known now"
expect 1 recheck

step "9. The decision log"
expect 0 log

if [ -e "$PROJECT/target" ] || [ -n "$(find "$PROJECT" -name 'BUILD_SCRIPT_RAN.marker')" ]; then
    echo "demo: something was built, which must never happen" >&2
    exit 1
fi

printf '\nDemo complete: a bad candidate was rejected, a good one was approved and\n'
printf 'verified, and the approval was invalidated when new evidence arrived.\n'
