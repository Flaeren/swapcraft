//! Embeds the product name and version in the exe.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "voidflare command line");
        res.set("FileDescription", "voidflare command line and MCP server");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe properties: {e}");
        }
    }
}
