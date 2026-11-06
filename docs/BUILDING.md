# MD Viewer: GPUI feasibility spike

A time-boxed spike to test whether MD Viewer can be remade in Rust with GPUI.
It is not the final port. Nothing here touches the Go app or `frontend/`.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/theme` | Colour and metric tokens (generated from `design/tokens.json` plus hand-written CSS values). |
| `crates/app` | The binary and window chrome. Depends on the other crates. |
| `crates/reader`, `crates/content`, `crates/platform` | Stub libraries filled in by other agents. They depend on `theme`, never on `app`. |
| `vendor/gpui-pre-windows` | `gpui-pre-windows 0.3.11` with one patch (see below). |

Pinned stack: `gpui-pre =0.3.11` and `gpui-pre-platform =0.3.11` (Zed's GPUI snapshot on crates.io, Apache-2.0), Rust 1.99.1 (`rust-toolchain.toml`).

## Build on Linux

Packages (Ubuntu 26.04): `build-essential clang lld cmake pkg-config libxkbcommon-x11-dev libxkbcommon-dev libwayland-dev libxcb1-dev libxcb-xkb-dev libx11-xcb-dev libfontconfig-dev libfreetype-dev libasound2-dev libssl-dev libvulkan1 mesa-vulkan-drivers`.

```sh
cd spike-gpui
cargo build --release
```

Run under Xvfb with software Vulkan (unset `WAYLAND_DISPLAY` so GPUI picks X11):

```sh
Xvfb :201 -screen 0 1920x1080x24 &
DISPLAY=:201 openbox &
env -u WAYLAND_DISPLAY DISPLAY=:201 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
  "$CARGO_TARGET_DIR/release/md-viewer-gpui"
```

## Cross-compile for Windows x86-64 (from Linux)

Needs `mingw-w64` and the Rust target; no Windows SDK or `fxc.exe`.

```sh
sudo apt-get install -y mingw-w64
rustup target add x86_64-pc-windows-gnu
cd spike-gpui
cargo build --release --target x86_64-pc-windows-gnu
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/md-viewer-gpui.exe" ../build/bin/md-viewer-gpui.exe
```

Never copy over `build/bin/md-viewer.exe` (the verified Go release).

### Why `vendor/gpui-pre-windows` exists

GPUI's Windows renderer precompiles its HLSL shaders with `fxc.exe` in `build.rs`, but that
step is gated on the *host* OS (`cfg(target_os = "windows")`), so a Linux host produces no
shader bytes and the release build fails to compile. The vendored copy compiles the embedded
HLSL when the app starts, using the system `d3dcompiler_47.dll` (the same compiler the upstream
debug build already uses). Cost: a short start-up delay. Risk: it has not been run on Windows.

## Release builds

The workspace's release profile is fat LTO, `opt-level = "s"` with `"z"` for diagrams and PDF, and
stripped symbols. `panic` stays `unwind`, as render panics are contained with `catch_unwind`.

Compiled-in panic messages hold source paths. Stable Cargo has no `trim-paths`, so a build for
publishing replaces those prefixes:

```sh
export RUSTFLAGS="--remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$HOME/.rustup=/rustup --remap-path-prefix=$(git rev-parse --show-toplevel)=/src"
cargo build --release --target x86_64-pc-windows-gnu
```

Check the result with `strings -a <exe> | grep -E "/home/|/Users/|C:.Users"`, which should print nothing.

## Regenerating theme tokens

```sh
python3 scripts/generate-theme.py
```

This rewrites `frontend/theme.css`, `theme_colours.go` and
`spike-gpui/crates/theme/src/generated.rs`. The first two must stay byte-identical.

## Test switches (environment variables)

These exist only in a build with `--features dev-harness`. A default build reads no `MDV_` variable
at all, so never ship one built with the feature: `MDV_SCRIPT` runs a script of commands, including
shell commands. They're all in `crates/app/src/harness.rs`, `crates/app/src/demo.rs` and
`crates/reader/src/ui/script.rs`.

`MDV_DEMO` runs the chrome on stub data and takes a comma list: `tabs`, `light`, `find`, `settings`,
`tools` (narrow overflow menu), `web`, `toast`, `toc100k` (a 100,000-heading outline).
`MDV_SIZE=800x600` sets the window size, `MDV_PLATFORM=mac` forces the macOS layout, and
`MDV_PERF=1` prints first-render and TOC row-build timings.

X11 note: GPUI only drops the window manager frame when a compositor is running and the window
manager advertises `_GTK_FRAME_EXTENTS` (xfwm4 does; Openbox does not). Without both, the
system frame stays and the app header sits inside it.
