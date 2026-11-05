//! Windows only: embeds the application manifest (per-monitor v2 DPI awareness, Common Controls 6),
//! the icon and the version information. Without a manifest Windows treats the process as DPI
//! unaware above 100 % scaling and stretches its whole window as a bitmap. Other targets do nothing.

fn main() {
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/app.manifest");
    println!("cargo:rerun-if-changed=../../../build/windows/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("windows/app.rc", embed_resource::NONE).manifest_required().expect("embedding the Windows resources");
    }
}
