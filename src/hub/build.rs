//! Embeds the voidflare icon and product name in the Windows exe. The window's own icon comes from
//! `src/main.rs`; this one is what Explorer and a pinned shortcut show.

fn main() {
    println!("cargo:rerun-if-changed=assets/voidflare.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/voidflare.ico");
        res.set("ProductName", "voidflare");
        res.set("FileDescription", "voidflare hub");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe icon: {e}");
        }
    }
}
