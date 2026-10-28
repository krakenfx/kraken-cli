# kraken-cli task runner — the single source of truth for every quality gate.
#
# Each gate is defined once, here, and `just ready` runs them all.

# Tool pins. CI caches the installed binaries; a bump here rebuilds them once.
CARGO_DENY_VERSION := "0.20.2"
CARGO_NEXTEST_VERSION := "0.9.140"
TAPLO_VERSION := "0.10.0"
# Pinned, not plain `nightly`: rustfmt.toml turns on comment and string
# wrapping, so an unpinned nightly would reformat the tree whenever it moved.
NIGHTLY_VERSION := "nightly-2026-08-20"

# Derived from Cargo.toml's rust-version so the gate can't drift from it.
MSRV := `sed -nE 's/^rust-version *= *"([0-9.]+)"$/\1/p' Cargo.toml`

# List available recipes.
default:
    @just --list

# Install the pinned npm-owned formatters (CI installs one profile per job).
install-tools:
    sh tools/npm-install.sh

# Full local gate before pushing.
ready: fmt-check lint-non-rust lint test catalog-check docs deny msrv duckdb
    @echo "✓ ready — local gates green"

# Format all code (rustfmt.toml is nightly-only; see NIGHTLY_VERSION).
fmt: install-nightly-rustfmt
    cargo +{{NIGHTLY_VERSION}} fmt
    just fmt-non-rust

# Check formatting without writing.
fmt-check: install-nightly-rustfmt
    cargo +{{NIGHTLY_VERSION}} fmt --check

# Best-effort: offline runs still format fine when the toolchain is present.
[private]
install-nightly-rustfmt:
    rustup toolchain install {{NIGHTLY_VERSION}} --profile minimal --component rustfmt 2>/dev/null || true

# Format every TOML file: manifests, lint config, tool config.
fmt-toml:
    cargo install --locked taplo-cli@{{TAPLO_VERSION}}
    RUST_LOG=ERROR taplo fmt

# Check TOML formatting without writing.
lint-toml:
    cargo install --locked taplo-cli@{{TAPLO_VERSION}}
    RUST_LOG=ERROR taplo fmt --check

# Sequential because both rewrite the same files: run in parallel, the slower
# one silently discards the other's fixes. markdownlint goes first because its
# fixes change content — stripping a space inside `code` shortens the line —
# and prettier must have the last word on where lines break.
#
# markdownlint's exit code is ignored here: it reports anything it cannot fix,
# some of which prettier then fixes (table alignment). This recipe applies
# every fix available; `lint-markdown` is what decides whether the tree passes.
[doc('Fix Markdown rule violations, then reflow the prose')]
fmt-markdown:
    node tools/run-npm-tool.mjs markdownlint --fix . || true
    node tools/run-npm-tool.mjs prettier --write '**/*.md'

# Both run even when the first fails, so one pass shows every problem.
[doc('Check Markdown formatting without writing')]
lint-markdown:
    #!/usr/bin/env sh
    set -eu
    cd "{{justfile_directory()}}"
    rc=0
    node tools/run-npm-tool.mjs prettier --check '**/*.md' || rc=1
    node tools/run-npm-tool.mjs markdownlint . || rc=1
    exit $rc

# Format every non-Rust file.
fmt-non-rust: fmt-toml fmt-markdown

# Check every non-Rust file.
lint-non-rust: lint-toml lint-markdown

# `-D warnings` stays an argument, not RUSTFLAGS: RUSTFLAGS enters crate
# fingerprints and would defeat CI's shared target/ cache.
# Not `--all-features`: that re-enables `record-duckdb` and pays the bundled
# libduckdb C++ build. Trade-off — the gated blocks go untyped here; `duckdb`
# covers them.
# Lint with clippy, warnings-as-errors (CI passes `json`).
lint format="human" features="":
    cargo clippy --locked --workspace --all-targets --features "{{features}}" --message-format={{format}} -- -D warnings

# Fix clippy findings that have machine-applicable suggestions.
fix features="":
    cargo clippy --fix --workspace --all-targets --features "{{features}}" --allow-dirty --allow-staged

# The catalog is generated, never edited: every `input_schema` comes from the
# command's own Rust type, so a flag rename reshapes it and cannot drift from it.
# `the_published_catalog_is_the_generated_one` fails when the checked-in file and
# this output disagree — that test is the reason to run this, not a reason to
# hand-edit the file.
# Regenerate the published command catalog (agents/tool-catalog.json).
catalog:
    #!/usr/bin/env bash
    set -euo pipefail
    # Held in a variable rather than redirected: a bare `>` truncates the catalog
    # before cargo runs, so a build failure would leave it empty.
    generated="$(cargo run --locked -q -p kraken-cli --bin kraken-mcp -- --emit-catalog)"
    printf '%s\n' "$generated" > agents/tool-catalog.json
    echo "✓ agents/tool-catalog.json regenerated"

# The unit suite already compares the two as JSON, so a changed command fails
# there. This compares bytes, which also catches a hand-edit or a formatting
# change that JSON equality lets through — the catalog is published, so what
# ships should be exactly what the tree generates.
# Check the published catalog is current.
catalog-check:
    #!/usr/bin/env bash
    set -euo pipefail
    generated="$(mktemp)"
    trap 'rm -f "${generated}"' EXIT
    cargo run --locked -q -p kraken-cli --bin kraken-mcp -- --emit-catalog > "${generated}"
    if ! diff -u agents/tool-catalog.json "${generated}"; then
      echo "error: agents/tool-catalog.json is stale — regenerate it with 'just catalog'" >&2
      exit 1
    fi
    echo "✓ agents/tool-catalog.json matches the command tree"

# No fail-fast anywhere: one run reports every failure yet still fails the gate.
# No `cargo test --doc` step: nextest skips doctests, but this workspace has none,
# so it was a second compilation pass that tested nothing. `doctest-tripwire`
# guards that assumption from the `docs` gate instead.
# Run the full test suite (CI passes `ci` for the JUnit report).
test profile="default" features="":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo install --locked cargo-nextest@{{CARGO_NEXTEST_VERSION}}
    cargo nextest run --locked --workspace --no-fail-fast --profile {{profile}} --features "{{features}}"

# A scan, not a compile: `test` runs no doctests, so a new one must not slip in
# unrun. If this fires, restore the `--doc` step or tag the fence `text`/`ignore`.
#
# Only an *opening* fence rustdoc would compile counts: an untagged one, or one
# tagged with a rustdoc attribute. A prose fence (```text, ```json) is not a
# doctest, and a bare closing fence is not an opening one — hence the state
# toggle, which a line-at-a-time grep cannot do.
[private]
doctest-tripwire:
    #!/usr/bin/env bash
    set -euo pipefail
    # awk reports a hit with a nonzero status, the opposite of the grep this
    # replaced, and xargs turns any batch's nonzero into 123 — so negate the
    # pipeline rather than the awk, and a hit in any batch still counts.
    if ! find crates -name '*.rs' -print0 | xargs -0 awk '
      FNR == 1 { inside = 0 }
      /^[[:space:]]*(\/\/\/|\/\/!)[[:space:]]*```/ {
        if (inside) { inside = 0; next }
        inside = 1
        info = $0
        sub(/^.*```/, "", info)
        gsub(/[[:space:],]+/, " ", info)
        if (info ~ /^ *$/ || info ~ /(^| )(rust|should_panic|no_run|compile_fail|edition[0-9]*)( |$)/) {
          print FILENAME ":" FNR ":" $0
          found = 1
        }
      }
      END { exit(found ? 1 : 0) }
    '; then
      echo "error: doctests found, but 'just test' runs none — see the note on the test recipe" >&2
      exit 1
    fi

# Private items included: internal docs are the audience of a bin-first crate.
# The doctest tripwire rides along here: it is a docs assertion, not a test one.
# Build docs with warnings-as-errors.
docs: doctest-tripwire
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items

# Separate from `lint`/`test` so an everyday check never pays the bundled libduckdb C++
# build for a backend that changes rarely.
# Lint and test the `record-duckdb` blocks.
duckdb: (lint "human" "record-duckdb") (test "default" "record-duckdb")

# Check licenses and advisories.
deny:
    cargo install --locked cargo-deny@{{CARGO_DENY_VERSION}}
    cargo deny check

# The second check pays another bundled libduckdb build: the MSRV also covers
# `cargo install --features record-duckdb`, the feature releases ship with.
# Verify the workspace builds on the MSRV from Cargo.toml.
msrv:
    rustup toolchain install {{MSRV}} --profile minimal
    RUSTUP_TOOLCHAIN={{MSRV}} cargo check --locked --workspace --all-targets
    RUSTUP_TOOLCHAIN={{MSRV}} cargo check --locked -p kraken-cli --features record-duckdb
