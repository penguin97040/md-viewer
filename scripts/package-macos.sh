#!/usr/bin/env bash
# Builds "MD Viewer.app" and a zip of it from release binaries. Runs on macOS (needs lipo,
# codesign and ditto).
#
#   scripts/package-macos.sh VERSION OUT_ZIP BINARY [BINARY...]
#
# With two binaries (arm64 and x86-64) they are joined into one universal executable. The app is
# signed ad hoc only: it is not notarised, so Gatekeeper asks the first time (see README.md).
set -euo pipefail

version=${1:?usage: package-macos.sh VERSION OUT_ZIP BINARY...}
out_zip=${2:?usage: package-macos.sh VERSION OUT_ZIP BINARY...}
shift 2
[ "$#" -ge 1 ] || { echo "no binary given" >&2; exit 2; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "version must look like 1.0.0, got: $version" >&2; exit 2; }

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/MD Viewer.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

if [ "$#" -eq 1 ]; then
  cp "$1" "$app/Contents/MacOS/md-viewer"
else
  lipo -create -output "$app/Contents/MacOS/md-viewer" "$@"
fi
chmod 755 "$app/Contents/MacOS/md-viewer"

# The version is numeric by the check above, so it is safe inside sed.
sed "s/@VERSION@/${version%%-*}/g" "$root/assets/macos/Info.plist.template" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp "$root/assets/macos/icon.icns" "$app/Contents/Resources/icon.icns"
cp "$root/LICENSE" "$root/THIRD-PARTY.md" "$app/Contents/Resources/"

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
lipo -archs "$app/Contents/MacOS/md-viewer"

mkdir -p "$(dirname "$out_zip")"
rm -f "$out_zip"
ditto -c -k --keepParent "$app" "$out_zip"
echo "wrote $out_zip"
