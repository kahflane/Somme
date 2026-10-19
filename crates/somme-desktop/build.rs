//! Gives the Windows program its icon, name and version, so Explorer and
//! signing tools show "Somme" rather than a bare file name.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icon.ico")
        .set("ProductName", "Somme")
        .set("FileDescription", "Somme")
        .set("InternalName", "Somme")
        .set("OriginalFilename", "Somme.exe");
    if let Err(err) = resource.compile() {
        panic!("cannot add the icon and version to Somme.exe: {err}");
    }
}
