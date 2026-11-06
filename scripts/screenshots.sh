#!/usr/bin/env bash
# Regenerates docs/screenshots/*.png (1280 x 800) from a Linux build of the app.
#
#   cargo build --release -p md-viewer
#   bash scripts/screenshots.sh [path/to/md-viewer]     # default: target/release/md-viewer
#
# Needs Xvfb, xdotool, ImageMagick (`import`, `convert`) and a Vulkan driver; on a machine
# without a GPU use Mesa's software one (mesa-vulkan-drivers, lavapipe). It starts its own X
# server on display :209 (override with MDV_SHOT_DISPLAY) and leaves your session alone.
#
# The shots drive the app with key presses, so keep these shortcuts in step with the README:
# Ctrl+Shift+D (theme), Page Down (scroll), Ctrl+F (find). The settings shot clicks the settings button in
# the header (the sliders icon); if the header layout changes, adjust GEAR_X and GEAR_Y below.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bin=${1:-$root/target/release/md-viewer}
out=$root/docs/screenshots
display=${MDV_SHOT_DISPLAY:-:209}
W=1280 H=800
GEAR_X=1119 GEAR_Y=22

for tool in Xvfb xdotool import convert; do
  command -v "$tool" >/dev/null || { echo "missing: $tool" >&2; exit 2; }
done
[ -x "$bin" ] || { echo "no executable at $bin" >&2; exit 2; }

work=$(mktemp -d)
pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT

Xvfb "$display" -screen 0 1600x1000x24 >/dev/null 2>&1 &
pids+=($!)
sleep 1

export DISPLAY=$display MDV_SIZE=${W}x${H} MDV_NO_SINGLE_INSTANCE=1
unset WAYLAND_DISPLAY
# Use the environment's Vulkan driver; fall back to Mesa's software one when there's no GPU.
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -f /usr/share/vulkan/icd.d/lvp_icd.json ]; then
  export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
fi
# Fresh settings every run, so the shots don't depend on the machine they're made on.
export XDG_CONFIG_HOME=$work/config HOME=$work/home
mkdir -p "$XDG_CONFIG_HOME" "$HOME"

# The last file opened is the one shown.
files=("$out/gear-list.md" "$out/kepler-track.md")

# shoot NAME KEYS...: opens the app, sends each key (or "type:text", "click:x,y", "wait:seconds"),
# then saves the window as NAME.png. A key may be any xdotool key name.
shoot() {
  local name=$1; shift
  "$bin" "${files[@]}" >"$work/app.log" 2>&1 &
  local pid=$!
  pids+=("$pid")
  local win=""
  for _ in $(seq 1 100); do
    win=$(xdotool search --pid "$pid" 2>/dev/null | tail -n1 || true)
    [ -n "$win" ] && break
    sleep 0.2
  done
  [ -n "$win" ] || { echo "$name: no window appeared (see below)" >&2; cat "$work/app.log" >&2; kill "$pid" 2>/dev/null; return 1; }
  xdotool windowmove "$win" 0 0
  sleep 3   # first layout, diagrams and maths
  xdotool mousemove 5 5   # keep the pointer off the document
  xdotool windowfocus "$win" 2>/dev/null || true
  local step
  for step in "$@"; do
    case $step in
      type:*)  xdotool type --delay 80 "${step#type:}" ;;
      click:*) IFS=, read -r cx cy <<<"${step#click:}"; xdotool mousemove "$cx" "$cy" click 1 ;;
      wait:*)  sleep "${step#wait:}" ;;
      *)       xdotool key --window "$win" "$step" ;;
    esac
    sleep 0.6
  done
  sleep 1
  xdotool mousemove 5 5
  sleep 0.3
  import -window "$win" "$work/$name.png"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  # Always exactly W x H, whatever the window manager did to the frame.
  convert "$work/$name.png" -crop "${W}x${H}+0+0" +repage -strip "$out/$name.png"
  echo "wrote docs/screenshots/$name.png ($(identify -format '%wx%h' "$out/$name.png"))"
}

shoot dark
shoot diagrams Next
shoot find ctrl+f "type:hut" Return
shoot settings "click:$GEAR_X,$GEAR_Y"
# Last, because the theme choice is saved in the settings the later shots would start from.
shoot light ctrl+shift+d Next
