#!/usr/bin/env bash
# Rebuild browser assets from the locked maintenance dependencies.
#   scripts/update-vendor.sh [--check]
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -gt 1 || (${1:-} != '' && ${1:-} != '--check') ]]; then
  echo 'Usage: scripts/update-vendor.sh [--check]' >&2
  exit 2
fi
vendor_tmp=$(mktemp -d)
trap 'rm -rf "$vendor_tmp"' EXIT
cp scripts/vendor/package{,-lock}.json "$vendor_tmp/"
npm ci --prefix "$vendor_tmp" --ignore-scripts --no-fund
npm audit --prefix "$vendor_tmp" --audit-level=info
node scripts/vendor/build.mjs "$vendor_tmp"
if [[ ${1:-} == '--check' ]]; then
  diff -qr frontend/vendor "$vendor_tmp/output"
  echo 'Bundled assets match the dependency lock.'
else
  rm -rf frontend/vendor/katex/fonts
  cp -R "$vendor_tmp/output/." frontend/vendor/
  echo 'Rebuilt Mermaid and KaTeX. Update component versions in THIRD-PARTY.md if needed.'
fi
