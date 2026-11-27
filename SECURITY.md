# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub's
[private vulnerability reporting](https://github.com/yigagilbert/depvigil/security/advisories/new).
Do not open a public issue for a suspected vulnerability.

Include what you found, how to reproduce it, and the version or commit you
tested. You will get a reply through the same advisory thread.

## Scope

depvigil reads dependency manifests and lockfiles and runs Cargo's resolution
commands in a temporary copy. It is designed never to build or run
third-party code. A way to make depvigil compile a dependency, run a build
script, or write outside its own `.depvigil` directory and temporary copies
is a security bug and should be reported as one.

## Supported versions

The project is in early development. Only the `main` branch is supported.
