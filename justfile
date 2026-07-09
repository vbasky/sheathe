# sheathe developer tasks — `just` command runner

_default:
    @just --list

# One-time after cloning: point git at the committed hooks (pre-commit auto-fmt)
setup:
    git config core.hooksPath .githooks
    @echo "→ core.hooksPath set to .githooks"

# Build the whole workspace
build:
    cargo build --workspace

# Build release with LTO
build-release:
    cargo build --workspace --release

# Run all tests
test:
    cargo test --workspace

# Run clippy with workspace lints, warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Format all code
fmt:
    cargo fmt --all

# Check formatting without changing files
fmt-check:
    cargo fmt --all --check

# Build documentation
docs:
    cargo doc --no-deps --document-private-items

# The full CI gate, locally: fmt + clippy + test + doc
check-all: fmt-check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

# Run the CLI, passing through args:  just run -- 2 3
run *args:
    cargo run -p sheathe -- {{args}}

# Fetch the real-media oracle corpus into corpus/media/ (see corpus/manifest.toml)
corpus:
    ./corpus/fetch.sh

# Package every corpus asset with sheathe and diff vs its oracle (status table)
oracle-corpus filter='':
    ./scripts/oracle_corpus.sh {{filter}}

# Differential-test sheathe vs Shaka Packager on an input (requires packager on PATH)
oracle input segment_seconds='6':
    ./scripts/shaka_oracle.sh {{input}} {{segment_seconds}}

# Audit advisories + licenses + bans (requires: cargo install cargo-deny)
deny:
    cargo deny check

# Release a version: bump → test → tag → push (CI builds binaries + GitHub Release).
# Usage: just release 0.2.0     (set PUBLISH=1 to also publish to crates.io)
release version:
    ./scripts/release.sh {{version}}

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update:
    cargo update
