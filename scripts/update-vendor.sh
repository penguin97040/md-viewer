#!/usr/bin/env bash
# Refreshes the bundled mermaid and KaTeX files from npm.
#   scripts/update-vendor.sh [mermaid-version] [katex-version]
set -euo pipefail
cd "$(dirname "$0")/.."
MERMAID=${1:-latest}
KATEX=${2:-latest}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
(cd "$tmp" && npm pack --silent "mermaid@$MERMAID" "katex@$KATEX" >/dev/null 2>&1)
mkdir -p "$tmp/m" "$tmp/k"
tar xzf "$tmp"/mermaid-*.tgz -C "$tmp/m"
tar xzf "$tmp"/katex-*.tgz -C "$tmp/k"

v=frontend/vendor
rm -rf "$v/katex/fonts"
mkdir -p "$v/katex/fonts"
gzip -9 -n -c "$tmp/m/package/dist/mermaid.min.js" > "$v/mermaid.min.js.gz"
gzip -9 -n -c "$tmp/k/package/dist/katex.min.js" > "$v/katex/katex.min.js.gz"
cp "$tmp"/k/package/dist/fonts/*.woff2 "$v/katex/fonts/"
# Keep only the woff2 font sources; the others are never needed.
sed -E 's/,url\([^)]*\.woff\) format\("woff"\)//g; s/,url\([^)]*\.ttf\) format\("truetype"\)//g' \
  "$tmp/k/package/dist/katex.min.css" > "$v/katex/katex.min.css"
cp "$tmp/m/package/LICENSE" "$v/LICENSE-mermaid.txt"
cp "$tmp/k/package/LICENSE" "$v/katex/LICENSE.txt"
echo "mermaid $(node -p "require('$tmp/m/package/package.json').version")," \
     "katex $(node -p "require('$tmp/k/package/package.json').version")"
echo "Update the versions in THIRD-PARTY.md."
