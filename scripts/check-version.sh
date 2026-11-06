#!/usr/bin/env bash
# Fails unless a release tag (v1.2.3) matches the version in crates/app/Cargo.toml, which is the
# version the app reports and that the macOS bundle and Windows resources are labelled with.
#   scripts/check-version.sh v1.0.0
set -euo pipefail
tag=${1:?usage: check-version.sh vX.Y.Z}
root=$(cd "$(dirname "$0")/.." && pwd)
want=${tag#v}
have=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/crates/app/Cargo.toml" | head -n1)
core=${want%%-*}
rc=$(sed -n 's/.*VALUE "ProductVersion", "\(.*\)"/\1/p' "$root/crates/app/windows/app.rc" | head -n1)
[ "$have" = "$want" ] || { echo "tag $tag does not match crates/app/Cargo.toml version $have" >&2; exit 1; }
[ "$rc" = "$core" ] || { echo "crates/app/windows/app.rc says $rc, expected $core" >&2; exit 1; }
echo "version $want ok"
