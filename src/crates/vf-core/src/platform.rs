//! What differs between Windows and Linux, in one place: program names, the settings folder, the label of this
//! platform in release file names.

use std::path::PathBuf;

/// `.exe` on Windows, nothing on Linux.
pub const EXE_SUFFIX: &str = std::env::consts::EXE_SUFFIX;

/// A program's file name on this platform: `exe("wordcraft-cli")` is `wordcraft-cli.exe` on Windows and
/// `wordcraft-cli` on Linux.
pub fn exe(stem: &str) -> String {
    format!("{stem}{EXE_SUFFIX}")
}

/// The label of this platform in the names of release files: `windows` (kept as it was before Linux, so hubs
/// up to 0.5 still find their files) or `linux-x86_64`.
pub const RELEASE_TAG: &str = if cfg!(windows) { "windows" } else { "linux-x86_64" };

/// The label in the names of the app and engine packs: `windows-x64` or `linux-x86_64`.
pub const PACK_TAG: &str = if cfg!(windows) { "windows-x64" } else { "linux-x86_64" };

/// Where SwapCraft keeps its settings (language, recipes, menus...): `%APPDATA%\voidflare` on Windows,
/// `$XDG_CONFIG_HOME/voidflare` (normally `~/.config/voidflare`) on Linux. The folder keeps the old name on both,
/// like the registry keys, so nothing has to move.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("voidflare"))
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("voidflare"))
    }
}

/// A file in [`data_dir`].
pub fn data_file(name: &str) -> Option<PathBuf> {
    data_dir().map(|d| d.join(name))
}

/// Opens a folder in the system's file manager (Explorer, or whatever `xdg-open` picks on Linux).
pub fn open_folder(dir: impl AsRef<std::path::Path>) {
    let dir = dir.as_ref();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

/// Shows a file in the file manager: selected in Explorer; on Linux its folder is opened.
pub fn reveal(file: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn();
    #[cfg(not(windows))]
    if let Some(dir) = file.parent() {
        open_folder(dir);
    }
}

/// On Linux a file unpacked from a ZIP keeps the permissions it was packed with (so programs stay runnable);
/// Windows has no such bits.
#[cfg(unix)]
pub fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_platform() {
        if cfg!(windows) {
            assert_eq!(exe("wordcraft-cli"), "wordcraft-cli.exe");
            assert_eq!(RELEASE_TAG, "windows");
        } else {
            assert_eq!(exe("wordcraft-cli"), "wordcraft-cli");
            assert_eq!(PACK_TAG, "linux-x86_64");
        }
        assert!(data_file("x.txt").map_or(true, |p| p.ends_with("voidflare/x.txt") || p.ends_with("voidflare\\x.txt")));
    }
}
