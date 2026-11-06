# Building MD Viewer

MD Viewer is a Rust workspace. The result is one executable. Rust is pinned by
`rust-toolchain.toml` (1.99.1), so `cargo` installs it on first use.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/app` | The binary: windows, tabs, chrome, settings, printing. Depends on the others. |
| `crates/reader` | Markdown parsing, the document model, layout and the reading view. |
| `crates/content` | Maths and diagrams as SVG (RaTeX, merman), with no GPUI types. |
| `crates/platform` | Tab-movement and native-window logic that doesn't need GPUI. |
| `crates/print` | The paginated light-theme PDF. |
| `crates/theme` | Colour and metric tokens, generated from `design/tokens.json`. |
| `vendor/gpui-pre-windows` | Zed's Windows backend for GPUI with the shader change described below. |
| `assets/` | Fonts (Geist and the derived MDV Serif), the icon, the macOS bundle files. |
| `scripts/` | Release, packaging and generator scripts (each has a header comment). |
| `docs/` | These notes and the README screenshots. |

Pinned stack: `gpui-pre =0.3.11` and `gpui-pre-platform =0.3.11` (a snapshot of Zed's GPUI
published on crates.io, Apache-2.0). Every dependency is pinned with `=` in the root
`Cargo.toml`, and `Cargo.lock` is committed. CI builds with `--locked`.

## Build on Linux

Packages (Ubuntu 24.04 or later):
`build-essential clang lld cmake pkg-config libxkbcommon-x11-dev libxkbcommon-dev libwayland-dev
libxcb1-dev libxcb-xkb-dev libx11-xcb-dev libfontconfig-dev libfreetype-dev libasound2-dev
libssl-dev libvulkan1 mesa-vulkan-drivers`.

```sh
cargo build --release
cargo test --workspace
```

To run under Xvfb with software Vulkan (unset `WAYLAND_DISPLAY` so GPUI picks X11):

```sh
Xvfb :201 -screen 0 1920x1080x24 &
DISPLAY=:201 openbox &
env -u WAYLAND_DISPLAY DISPLAY=:201 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
  target/release/md-viewer file.md
```

## Windows

### Official releases

The release workflow builds on a Windows runner for `x86_64-pc-windows-msvc` with
`--features precompiled-shaders`. `build.rs` in `vendor/gpui-pre-windows` then compiles the HLSL
shaders with `fxc.exe` from the Windows SDK and links the bytecode into the executable. The app
never compiles shaders when it starts and writes nothing under `%LOCALAPPDATA%`.

Two guards stop a release shipping the other way:

- `MDV_REQUIRE_PRECOMPILED_SHADERS=1` (set by the workflow) makes the build fail if the feature is
  off.
- `scripts/check-release-binary.sh windows <exe>` runs after the build and fails unless the file
  holds the 18 compiled shaders (`DXBC` blobs) and has no reference to `D3DCompile`,
  `d3dcompiler_47` or `shader-cache`. The same script fails on build-machine paths.

To make the same build by hand on Windows (Visual Studio Build Tools and the Windows SDK, which
provides `fxc.exe`; `GPUI_FXC_PATH` points at it if it isn't found automatically):

```sh
cargo build --release --target x86_64-pc-windows-msvc -p md-viewer --features precompiled-shaders
```

### Cross-build from Linux (testing only)

Needs `mingw-w64` and the Rust target (listed in `rust-toolchain.toml`). There is no Windows SDK
or `fxc.exe` on Linux, so this build compiles the shaders when the app starts with the system
`d3dcompiler_47.dll` and keeps compiled bytecode in `%LOCALAPPDATA%\md-viewer\shader-cache`
(`MDV_NO_SHADER_DISK=1` turns the store off). It is for trying changes on a Windows machine. Don't
publish it.

```sh
sudo apt-get install -y mingw-w64
cargo build --release --target x86_64-pc-windows-gnu -p md-viewer
```

Check the build still compiles with `cargo check --target x86_64-pc-windows-gnu -p md-viewer`.

### What `vendor/gpui-pre-windows` changes

Upstream precompiles the shaders in `build.rs` only when the build machine is Windows and the
build isn't a debug one, and has no fallback, so a Linux host can't produce a Windows build. The
copy here (0.3.11, taken from the published crate) differs in these ways:

1. The `precompiled-shaders` feature chooses between the two ways of getting shader bytecode:
   built in with `fxc.exe` (feature on) or compiled when the app starts, shared between windows
   and stored on disk (feature off).
2. `build.rs` honours `MDV_REQUIRE_PRECOMPILED_SHADERS` and runs `fxc.exe` only for the feature.
3. Optional timing output for `MDV_TIMING` (a file path) used when measuring window start-up and
   tab dragging.

Not verified without Windows: that the `fxc.exe` path builds and runs (the binding-generation code
is upstream's, unchanged, and the Rust side was compile-checked here with stand-in bytecode), and
that the msvc build behaves like the gnu one the owner has tested.

## macOS

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target aarch64-apple-darwin -p md-viewer
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target x86_64-apple-darwin -p md-viewer
bash scripts/package-macos.sh 1.0.0 MD-Viewer-macos.zip \
  target/aarch64-apple-darwin/release/md-viewer target/x86_64-apple-darwin/release/md-viewer
```

`scripts/package-macos.sh` makes `MD Viewer.app` from `assets/macos/Info.plist.template` (document
types for `.md` and `.markdown`) and `assets/macos/icon.icns`, joins the two builds into one
universal binary, signs it ad hoc and zips it. It is not notarised. `icon.icns` is made from
`assets/appicon.png` by `python3 scripts/make-icns.py`.

GPUI's Metal shaders are compiled at run time (`runtime_shaders`), so the build needs no Xcode Metal
toolchain.

**macOS has never been run.** The app does compile for `aarch64-apple-darwin` as far as
`cargo check` can tell from Linux, but only with a stand-in for one generated Metal file that the
GPUI build script produces on a Mac. Nobody has launched it, opened a file with it or tested
the packaged bundle. The first release build on a macOS runner is the first real compile.

## Release builds

The release profile is fat LTO with `opt-level = "s"` (and `"z"` for diagrams and PDF code) and
stripped symbols; see the comment in `Cargo.toml` for the measurements. `panic` stays `unwind`,
since render panics are contained with `catch_unwind`.

Compiled-in panic messages hold source paths. Stable Cargo has no `trim-paths`, so a build for
publishing replaces those prefixes, as the release workflow does:

```sh
export RUSTFLAGS="--remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$HOME/.rustup=/rustup --remap-path-prefix=$(git rev-parse --show-toplevel)=/src"
cargo build --release --target x86_64-unknown-linux-gnu -p md-viewer
bash scripts/check-release-binary.sh other target/x86_64-unknown-linux-gnu/release/md-viewer
```

### Releasing

1. Set the version in `crates/app/Cargo.toml` and in `crates/app/windows/app.rc` (all four
   places), and update the README if needed.
2. Tag `vX.Y.Z` and push the tag. `scripts/check-version.sh` stops the build if the tag and the
   files disagree.
3. `.github/workflows/release.yml` runs the dependency checks, then builds Windows (x86-64),
   macOS (universal, zipped `.app`) and Linux (x86-64, `.tar.gz`), runs the binary checks and
   attaches the files, `SHA256SUMS.txt` and build attestations to the release. Only the last job
   has write permission.

## Dependency policy

CI (`.github/workflows/dependencies.yml`) runs `cargo test` on Windows, macOS and Linux, then
`cargo audit` (RustSec advisories) and `cargo deny check` (advisories, licences, sources and
duplicate versions, configured in `deny.toml`). It also runs weekly, since advisories appear
without any change here. Every exception in `deny.toml` carries a reason; remove it when the cause
goes away. `python3 scripts/generate-licences.py` rewrites the crate list in `THIRD-PARTY.md`, and CI
fails if it's stale.

### Risks to watch

- **GPUI changes weekly.** The `gpui-pre` crates are snapshots of Zed's main branch, pinned
  exactly. Upgrade deliberately, on a branch, never in the week of a release: bump the snapshot,
  rebase the `vendor/gpui-pre-windows` changes onto the new crate, then run the tests and the
  platform checks (`cargo check` for Windows and macOS, a real run on Windows). Staying on one
  snapshot is safe, since nothing is fetched at run time; the cost of waiting is only that fixes
  to GPUI itself, and to security issues in its dependencies, arrive late.
- **merman is an alpha** (`0.10.0-alpha.1`). It draws the diagrams in release builds. Keep it pinned
  exactly, keep the guards that catch a panic or a runaway render so one diagram fails and not the
  app, move to its first stable release when it appears, and re-run the diagram samples on each
  bump.

## Regenerating theme tokens

```sh
python3 scripts/generate-theme.py
```

This rewrites `crates/theme/src/generated.rs` from `design/tokens.json`. CI regenerates it and
fails if the committed file differs.

## Fonts

Geist, Geist Mono and Geist Serif live in `assets/fonts`. Geist Serif has only one weight and no
italic, so `scripts/derive-serif.py` builds the MDV Serif bold and italic faces from it (needs
fonttools and skia-pathops). Licence: [THIRD-PARTY.md](../THIRD-PARTY.md).

## Screenshots

`docs/screenshots/*.png` are 1280 x 800 and are made by `scripts/screenshots.sh`; see the header
of that script for what it needs. Regenerate them after interface changes.

## Environment variables for testing

`MDV_SIZE=1280x800` sets the window size. `MDV_NO_SINGLE_INSTANCE=1` stops a second launch handing
its files to a running copy. `MDV_PLATFORM=mac` forces the macOS layout. `MDV_PERF=1` prints
timings. `MDV_SCRIPT=<file>` and `MDV_RESULTS=<file>` run a scripted session (see
`crates/reader/src/ui/script.rs`). `MDV_DEMO` takes a comma list: `light`, `find`, `settings`, `tools`
(narrow overflow menu), `web`, `toast`. Settings are read from and saved to the real config folder,
so set `XDG_CONFIG_HOME` to a scratch folder when testing on Linux.

X11 note: GPUI only drops the window manager frame when a compositor is running and the window
manager advertises `_GTK_FRAME_EXTENTS` (xfwm4 does; Openbox does not). Without both, the system
frame stays and the app header sits inside it.
