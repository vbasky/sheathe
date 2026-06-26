#!/usr/bin/env bash
#
# Release sheathe.
#
# Binary builds + the GitHub Release happen automatically in CI
# (.github/workflows/release.yml) when the tag is pushed. This script bumps the
# workspace version, tags, pushes, and (optionally) publishes to crates.io.
#
# Usage:   ./scripts/release.sh <version>          e.g. ./scripts/release.sh 0.2.0
#          PUBLISH=1 ./scripts/release.sh 0.2.0     also publish to crates.io
#
# Prerequisites: on `main`, clean working tree, CHANGELOG.md updated for <version>.

set -euo pipefail

VERSION="${1:?usage: scripts/release.sh <version>   e.g. 0.2.0}"
TAG="v${VERSION}"
PUBLISH="${PUBLISH:-0}"

cd "$(git rev-parse --show-toplevel)"

# Crates in dependency order: a crate must follow everything it depends on.
CRATES=(
    sheathe-core
    sheathe-crypto
    sheathe-dash
    sheathe-hls
    sheathe-mp4
    sheathe-ts
    sheathe-es
    sheathe-cli
    sheathe
)

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

# ── commit, tag, push (triggers CI binary build + GitHub Release) ───────────
git add Cargo.toml crates/*/Cargo.toml CHANGELOG.md
git commit -m "release: ${TAG}"
git tag -a "${TAG}" -m "sheathe ${VERSION}"
git push origin main
git push origin "${TAG}"
echo "==> tag pushed — CI is building binaries and creating the GitHub Release"

# ── optional: publish to crates.io in dependency order ──────────────────────
if [ "$PUBLISH" = "1" ]; then
    for c in "${CRATES[@]}"; do
        echo "==> cargo publish ${c}@${VERSION}"
        cargo publish -p "${c}"
    done
    echo "✓ published sheathe ${VERSION} to crates.io"
fi

echo "✓ released sheathe ${TAG}"
