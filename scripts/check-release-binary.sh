#!/usr/bin/env bash
# Checks a release executable before it is published.
#
#   scripts/check-release-binary.sh windows <md-viewer.exe> [shader-report]
#   scripts/check-release-binary.sh other <binary>
#
# Both kinds: no build-machine paths compiled into the file (see docs/BUILDING.md).
# Windows also: the Direct3D shaders were precompiled. A build that compiles them when the app
# starts imports D3DCompile and keeps a "shader-cache" folder; a precompiled build holds 9 shader
# pairs as DXBC blobs (the build fails if any is empty or not DXBC) and has neither. Exits non-zero on the first failure.
set -euo pipefail

kind=${1:?usage: check-release-binary.sh windows|other FILE}
file=${2:?usage: check-release-binary.sh windows|other FILE}
[ -f "$file" ] || { echo "no such file: $file" >&2; exit 2; }

fail=0
bad() { echo "FAIL: $*" >&2; fail=1; }

# Counts matches without tripping pipefail when there are none.
count() { { LC_ALL=C grep -aoE "$1" "$file" || true; } | wc -l | tr -d ' '; }

leaks=$(count '(/home/[A-Za-z0-9_.-]+|/Users/[A-Za-z0-9_.-]+|[A-Za-z]:.Users.[A-Za-z0-9_.-]+|/runner/work|/github/workspace)')
[ "$leaks" -eq 0 ] || bad "$leaks build-machine path(s) compiled in (check the --remap-path-prefix flags)"

if [ "$kind" = windows ]; then
  runtime=$(count 'D3DCompile|shader-cache|d3dcompiler_47')
  [ "$runtime" -eq 0 ] || bad "$runtime reference(s) to the run-time shader compiler or shader store; build with --features precompiled-shaders"
  # 9 vertex and 9 pixel shaders are built, but subpixel_sprite_vertex only calls
  # monochrome_sprite_vertex, so both compile to the same bytes and the linker keeps one copy.
  # That leaves 17 distinct blobs. If the build wrote its distinct count (MDV_SHADER_REPORT, passed
  # here as the third argument), that count is used instead, so the check follows the shaders.
  min_blobs=17
  report=${3:-}
  if [ -n "$report" ]; then
    [ -f "$report" ] || { echo "no such shader report: $report" >&2; exit 2; }
    min_blobs=$(tr -d '[:space:]' < "$report")
    case $min_blobs in ''|*[!0-9]*) echo "bad shader report: $report" >&2; exit 2;; esac
  fi
  blobs=$(count 'DXBC')
  [ "$blobs" -ge "$min_blobs" ] || bad "only $blobs DXBC shader blob(s) found, expected at least $min_blobs distinct ones"
  [ "$fail" -eq 0 ] && echo "shaders: $blobs DXBC blobs, no run-time compiler reference"
fi

[ "$fail" -eq 0 ] && echo "ok: $file"
exit "$fail"
