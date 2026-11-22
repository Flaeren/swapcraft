//! SwapCraft's resident helper (`swapcraft-tray`, the drop bar and the tray icon): finding it next to the hub,
//! starting it with Windows, and starting and stopping it. Windows only; elsewhere everything says "not supported".
//!
//! The start with Windows is one value in the person's own `Run` key (no administrator rights), written and read with
//! `reg.exe` like the right-click menu is.

use std::path::{Path, PathBuf};

pub const SUPPORTED: bool = cfg!(windows);

/// The helper's program name.
pub const EXE_NAME: &str = if cfg!(windows) { "swapcraft-tray.exe" } else { "swapcraft-tray" };

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "SwapCraft";
/// The class of the helper's main window: how to tell it runs, and how to ask it to quit.
#[cfg(windows)]
const WINDOW_CLASS: &str = "SwapCraftTray";

/// The helper that sits next to the hub program `hub_exe`, if it is there.
pub fn exe_beside(hub_exe: &Path) -> Option<PathBuf> {
    let p = hub_exe.with_file_name(EXE_NAME);
    p.is_file().then_some(p)
}

/// `reg.exe` with the arguments, no console window; its standard output when it succeeded.
fn reg(args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("reg");
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// The text a `reg query /v <name>` printed for the value, without the quotes around a path.
fn parse_reg_value(output: &str, name: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|l| {
        let rest = l.strip_prefix(name)?;
        let data = rest.trim_start().strip_prefix("REG_SZ")?.trim();
        Some(data.trim_matches('"').to_owned())
    })
}

fn autostart_target_named(name: &str) -> Option<String> {
    if !SUPPORTED {
        return None;
    }
    reg(&["query", RUN_KEY, "/v", name]).ok().and_then(|o| parse_reg_value(&o, name))
}

fn set_autostart_named(name: &str, exe: &Path, on: bool) -> Result<(), String> {
    if !SUPPORTED {
        return Err("Starting with the computer is only available on Windows.".to_owned());
    }
    if on {
        reg(&["add", RUN_KEY, "/v", name, "/t", "REG_SZ", "/d", &format!("\"{}\"", exe.display()), "/f"]).map(|_| ())
    } else if autostart_target_named(name).is_some() {
        reg(&["delete", RUN_KEY, "/v", name, "/f"]).map(|_| ())
    } else {
        Ok(())
    }
}

/// Whether the helper starts with Windows.
pub fn autostart_on() -> bool {
    autostart_target_named(RUN_VALUE).is_some()
}

/// Whether the start with Windows points at `exe` (a portable folder that was moved leaves it pointing nowhere).
pub fn autostart_points_to(exe: &Path) -> bool {
    autostart_target_named(RUN_VALUE).is_some_and(|t| Path::new(&t) == exe)
}

pub fn set_autostart(exe: &Path, on: bool) -> Result<(), String> {
    set_autostart_named(RUN_VALUE, exe, on)
}

/// The helper's window, if it is running (one copy per person: a second start exits at once).
#[cfg(windows)]
fn window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::HSTRING;
    unsafe { FindWindowW(&HSTRING::from(WINDOW_CLASS), None).ok() }
}

pub fn running() -> bool {
    #[cfg(windows)]
    {
        window().is_some()
    }
    #[cfg(not(windows))]
    false
}

/// Starts the helper if it is not running.
pub fn start(exe: &Path) -> Result<(), String> {
    if !SUPPORTED {
        return Err("The drop bar is only available on Windows.".to_owned());
    }
    if running() {
        return Ok(());
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Asks the running helper to quit (it takes its icon off the taskbar first).
pub fn stop() {
    #[cfg(windows)]
    if let Some(w) = window() {
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        unsafe {
            let _ = PostMessageW(Some(w), WM_CLOSE, Default::default(), Default::default());
        }
    }
}

/// Turns the drop bar on: it starts with Windows, and starts now.
pub fn enable(exe: &Path) -> Result<(), String> {
    set_autostart(exe, true)?;
    start(exe)
}

/// Turns the drop bar off: no start with Windows, and the running copy quits.
pub fn disable() -> Result<(), String> {
    set_autostart(Path::new(""), false)?;
    stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_value_out_of_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    SwapCraft    REG_SZ    \"D:\\My Apps\\SwapCraft\\swapcraft-tray.exe\"\r\n\r\n";
        assert_eq!(parse_reg_value(out, "SwapCraft").as_deref(), Some(r"D:\My Apps\SwapCraft\swapcraft-tray.exe"));
        assert_eq!(parse_reg_value(out, "Other"), None);
    }

    /// Writes, reads and removes a value of its own in the real Run key (never the one a person uses).
    #[cfg(windows)]
    #[test]
    fn a_start_with_windows_value_is_written_read_and_removed() {
        let name = format!("SwapCraftTest{}", std::process::id());
        let exe = Path::new(r"C:\Program Files\Some Folder\swapcraft-tray.exe");
        assert!(autostart_target_named(&name).is_none());
        set_autostart_named(&name, exe, true).unwrap();
        assert_eq!(autostart_target_named(&name).as_deref(), Some(exe.to_str().unwrap()), "the path with spaces comes back whole");
        set_autostart_named(&name, exe, false).unwrap();
        assert!(autostart_target_named(&name).is_none());
        set_autostart_named(&name, exe, false).unwrap(); // removing what is not there is fine
    }
}
