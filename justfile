# kraken-cli task runner — the single source of truth for every quality gate.
#
# CI runs these same recipes, so a green `just ready` means a green pipeline:
# each gate is defined exactly once, here.

# Tool pins. CI caches the installed binaries; a bump here rebuilds them once.
CARGO_SORT_VERSION := "2.1.4"
CARGO_DENY_VERSION := "0.20.2"
CARGO_NEXTEST_VERSION := "0.9.140"

# Derived from Cargo.toml's rust-version so the gate can't drift from it.
MSRV := `sed -nE 's/^rust-version *= *"([0-9.]+)"$/\1/p' Cargo.toml`

# List available recipes.
default:
    @just --list

# Full local gate — run before pushing. Mirrors the CI `test` stage 1:1.
ready: fmt-check sort-check lint test docs deny msrv
    @echo "✓ ready — matches the CI test stage"

# Format all code (nightly: group_imports in rustfmt.toml is nightly-only).
fmt: install-nightly-rustfmt
    cargo +nightly fmt

# Check formatting without writing (CI: test-fmt).
fmt-check: install-nightly-rustfmt
    cargo +nightly fmt --check

# Best-effort: offline runs still format fine when nightly is already present.
[private]
install-nightly-rustfmt:
    rustup toolchain install nightly --profile minimal --component rustfmt 2>/dev/null || true

# Check Cargo.toml key ordering in every workspace crate (CI: test-sort).
sort-check:
    cargo install --locked cargo-sort@{{CARGO_SORT_VERSION}}
    cargo sort --check --workspace

# `-D warnings` stays an argument, not RUSTFLAGS: RUSTFLAGS enters crate
# fingerprints and would defeat CI's shared target/ cache.
# Lint with clippy, warnings-as-errors (CI: test-clippy, passes `json`).
lint format="human":
    cargo clippy --locked --workspace --all-targets --all-features --message-format={{format}} -- -D warnings

# Fix clippy findings that have machine-applicable suggestions.
fix:
    cargo clippy --fix --workspace --all-targets --all-features --allow-dirty --allow-staged

# No fail-fast anywhere: one run reports every failure yet still fails the
# gate. Doctests are a second step because nextest doesn't execute them.
# Run the full test suite (CI: test-unit, passes `ci` for the JUnit report).
test profile="default":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo install --locked cargo-nextest@{{CARGO_NEXTEST_VERSION}}
    rc=0
    cargo nextest run --locked --workspace --no-fail-fast --profile {{profile}} || rc=$?
    cargo test --doc --locked --workspace --no-fail-fast || rc=$?
    exit "$rc"

# Private items included: internal docs are the audience of a bin-first crate.
# Build docs with warnings-as-errors (CI: test-docs).
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items

# Check licenses and advisories (CI: test-deny).
deny:
    cargo install --locked cargo-deny@{{CARGO_DENY_VERSION}}
    cargo deny check

# Verify the workspace builds on the MSRV from Cargo.toml (CI: test-msrv).
msrv:
    rustup toolchain install {{MSRV}} --profile minimal
    RUSTUP_TOOLCHAIN={{MSRV}} cargo check --locked --workspace --all-targets
