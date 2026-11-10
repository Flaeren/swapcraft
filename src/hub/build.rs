//! Embeds the SwapCraft icon and product name in the Windows exe. The window's own icon comes from
//! `src/main.rs`; this one is what Explorer, the right-click menus and a pinned shortcut show.

fn main() {
    println!("cargo:rerun-if-changed=assets/swapcraft.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/swapcraft.ico");
        res.set("ProductName", "SwapCraft");
        res.set("FileDescription", "SwapCraft");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe icon: {e}");
        }
    }
}
