//! Embeds the app's icon and name in the exe (Explorer, pinned shortcuts).

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("ProductName", "voidflare Audio Converter");
        res.set("FileDescription", "Audio Converter");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe icon: {e}");
        }
    }
}
