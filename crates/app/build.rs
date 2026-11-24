//! Windows resources for Sonkore.exe. The interface (Slint) compiles in
//! crates/ui.

fn main() {
    windows_resources();
}

/// Windows: the icon Explorer and the taskbar show for Sonkore.exe, and its
/// version (sonkore.rc). Needs the Windows SDK's rc.exe (the MSVC build has it).
fn windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=sonkore.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/sonkore.ico");
    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let mut part = || parts.next().unwrap_or(0);
    let macros = [
        format!("SONKORE_MAJOR={}", part()),
        format!("SONKORE_MINOR={}", part()),
        format!("SONKORE_PATCH={}", part()),
        format!("SONKORE_VERSION=\"{version}\""),
    ];
    if let Err(e) = embed_resource::compile("sonkore.rc", macros.iter()).manifest_optional() {
        println!("cargo:warning=Windows resources (icon, version) not embedded: {e:?}");
    }
}
