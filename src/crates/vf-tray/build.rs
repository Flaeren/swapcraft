//! Embeds the SwapCraft icon in the exe (the tray icon is loaded from it) and the product name.

fn main() {
    println!("cargo:rerun-if-changed=../../hub/assets/swapcraft.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../hub/assets/swapcraft.ico");
        res.set("ProductName", "SwapCraft");
        res.set("FileDescription", "SwapCraft tray");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe icon: {e}");
        }
    }
}
