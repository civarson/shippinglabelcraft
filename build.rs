// Embeds the app icon (and version info) into LabelCraft.exe so Explorer, the desktop
// and shortcuts show it, not only the running window.
fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/labelcraft.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/windows/labelcraft.ico");
        res.set("ProductName", "LabelCraft");
        res.set("FileDescription", "LabelCraft: crisp shipping labels, ready to print");
        if let Err(e) = res.compile() {
            // A missing resource compiler shouldn't stop the build; the exe just has no icon.
            println!("cargo:warning=couldn't embed the Windows icon: {e}");
        }
    }
}
