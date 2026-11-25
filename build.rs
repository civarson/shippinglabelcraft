// Embeds the app icon (and version info) into ShippingLabelCraft.exe so Explorer, the desktop
// and shortcuts show it, not only the running window.
fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/shippinglabelcraft.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/windows/shippinglabelcraft.ico");
        res.set("ProductName", "ShippingLabelCraft");
        res.set("FileDescription", "ShippingLabelCraft: crisp shipping labels, ready to print");
        if let Err(e) = res.compile() {
            // A missing resource compiler shouldn't stop the build; the exe just has no icon.
            println!("cargo:warning=couldn't embed the Windows icon: {e}");
        }
    }
}
