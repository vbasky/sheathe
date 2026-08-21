#!/usr/bin/env bash
#
# Release sheathe.
#
# This script bumps versions, commits, tags, and pushes. Binary builds, the
# GitHub Release, and crates.io publish happen in CI
# (`.github/workflows/release.yml`) when the tag is pushed.
#
# Usage:   ./scripts/release.sh <version>          e.g. ./scripts/release.sh 0.6.0
#
# Prerequisites: on `main`, clean working tree, CHANGELOG.md updated for <version>.

set -euo pipefail

VERSION="${1:?usage: scripts/release.sh <version>   e.g. 0.6.0}"
TAG="v${VERSION}"

cd "$(git rev-parse --show-toplevel)"

# ── pre-flight ──────────────────────────────────────────────────────────────
[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || { echo "✗ not on main"; exit 1; }
[ -z "$(git status --porcelain)" ]                || { echo "✗ working tree not clean — commit or stash first"; exit 1; }
git rev-parse "$TAG" >/dev/null 2>&1              && { echo "✗ tag $TAG already exists"; exit 1; }

echo "==> releasing sheathe ${VERSION}"
cargo test --workspace

# ── bump versions: each crate's package version + internal path-dep pins ─────
for f in crates/*/Cargo.toml; do
    perl -i -pe "s/^version = \"[^\"]+\"/version = \"${VERSION}\"/" "$f"
done
# Internal dependency pins in the root manifest: `{ path = "crates/..", version = "X" }`.
perl -i -pe "s/(path = \"crates\/[^\"]+\", version = \")[^\"]+/\${1}${VERSION}/g" Cargo.toml

cargo build --workspace   # validate the manifests compile before tagging

# ── commit, tag, push (triggers CI: binaries, GitHub Release, crates.io) ────
git add Cargo.toml Cargo.lock crates/*/Cargo.toml CHANGELOG.md
git commit -m "release: ${TAG}"
git tag -a "${TAG}" -m "sheathe ${VERSION}"
git push origin main
git push origin "${TAG}"
echo "==> tag pushed — CI is building binaries, creating the GitHub Release, and publishing crates.io"

echo "✓ released sheathe ${TAG}"
