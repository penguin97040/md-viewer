//! Windows only: embeds the application manifest (per-monitor v2 DPI awareness, Common Controls 6),
//! the icon and the version information. Without a manifest Windows treats the process as DPI
//! unaware above 100 % scaling and stretches its whole window as a bitmap. Other targets do nothing.
//!
//! GPUI's own build script links a manifest (resource 1) into every binary that uses it, and it
//! already asks for per-monitor v2 DPI awareness and Common Controls 6. The MSVC linker refuses two
//! manifests (CVT1100), so there ours is left out and GPUI's is the only one. The GNU linker only
//! warns and keeps both, so a cross-build still carries ours as well.

fn main() {
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/app.manifest");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let result = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            embed_resource::compile("windows/app.rc", embed_resource::ParamsMacros(["MDV_NO_MANIFEST"]))
        } else {
            embed_resource::compile("windows/app.rc", embed_resource::NONE)
        };
        result.manifest_required().expect("embedding the Windows resources");
    }
}
