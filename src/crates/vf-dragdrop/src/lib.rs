//! "Convert here with voidflare" in the menu File Explorer shows when files are dragged with the right mouse button
//! and dropped on a folder (where it offers Copy here / Move here).
//!
//! Explorer only takes COM objects there (a drag-and-drop handler: `IShellExtInit` + `IContextMenu`, registered under
//! `Directory\shellex\DragDropHandlers`), so this is a DLL that Explorer loads into its own process. It is kept
//! small on purpose: it reads `dragmenu.tsv` (see [`config`]), adds a submenu with the conversions that fit every
//! dragged file, and on a click writes the file list to `%TEMP%` and starts
//! `voidflare.exe --run <action> --to <folder> --list <file>`. No conversion code runs inside Explorer, and nothing
//! here panics across the COM boundary (every entry point is wrapped in `catch_unwind`).

pub mod config;

#[cfg(windows)]
mod handler;

#[cfg(windows)]
mod icon;

#[cfg(windows)]
pub use handler::{CLSID, DllCanUnloadNow, DllGetClassObject};

/// The list file the DLL hands to `voidflare.exe --list`: one path per line, UTF-8.
pub fn write_list(dir: &std::path::Path, files: &[std::path::PathBuf]) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = dir.join(format!("drop-{}-{stamp}.txt", std::process::id()));
    let text: Vec<String> = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
    std::fs::write(&path, text.join("\n"))?;
    Ok(path)
}

/// A Windows command line from arguments, quoted the way `CommandLineToArgvW` and the Rust runtime read it back:
/// every argument in quotes; backslashes are doubled only where a quote follows them (an inner quote, or the
/// closing one), and an inner quote gets a backslash.
pub fn command_line(args: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push('"');
        let mut slashes = 0;
        for c in arg.chars() {
            if c == '\\' {
                slashes += 1;
                continue;
            }
            if c == '"' {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
            } else {
                out.push_str(&"\\".repeat(slashes));
            }
            out.push(c);
            slashes = 0;
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::command_line;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn command_line_quoting() {
        assert_eq!(command_line(&a(&[r"C:\a b\x.exe", "--run"])), r#""C:\a b\x.exe" "--run""#);
        assert_eq!(command_line(&a(&[r#"recipe:My "best""#])), r#""recipe:My \"best\"""#);
        assert_eq!(command_line(&a(&[r"D:\drop\"])), r#""D:\drop\\""#, "a trailing backslash must not escape the closing quote");
        assert_eq!(command_line(&a(&[r#"a\"b"#])), r#""a\\\"b""#);
    }
}
