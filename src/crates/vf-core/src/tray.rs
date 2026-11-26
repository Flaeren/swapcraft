//! SwapCraft's resident helper (`swapcraft-tray`, the drop bar and the tray icon): finding it next to the hub,
//! starting it with Windows, and starting and stopping it. Windows only; elsewhere everything says "not supported".
//!
//! The start with Windows is one value in the person's own `Run` key (no administrator rights), written and read with
//! `reg.exe` like the right-click menu is.

use std::path::{Path, PathBuf};

/// Windows and Linux (X11) have the drop bar. The Mac one is not there yet (it needs a Mac to be tried on): until then
/// the hub does not offer it, and the Mac package does not carry the helper.
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));

/// The helper's program name.
pub const EXE_NAME: &str = if cfg!(windows) { "swapcraft-tray.exe" } else { "swapcraft-tray" };

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// The name of the start-with-the-computer entry (a registry value on Windows, `swapcraft.desktop` on Linux).
const RUN_VALUE: &str = "SwapCraft";
/// The class of the helper's main window: how to tell it runs, and how to ask it to quit.
#[cfg(windows)]
const WINDOW_CLASS: &str = "SwapCraftTray";

/// The helper that sits next to the hub program `hub_exe`, if it is there.
pub fn exe_beside(hub_exe: &Path) -> Option<PathBuf> {
    crate::layout::helper(hub_exe, EXE_NAME)
}

/// `reg.exe` with the arguments, no console window; its standard output when it succeeded.
#[cfg(windows)]
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
#[cfg(any(windows, test))]
fn parse_reg_value(output: &str, name: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|l| {
        let rest = l.strip_prefix(name)?;
        let data = rest.trim_start().strip_prefix("REG_SZ")?.trim();
        Some(data.trim_matches('"').to_owned())
    })
}

#[cfg(windows)]
fn autostart_target_named(name: &str) -> Option<String> {
    reg(&["query", RUN_KEY, "/v", name]).ok().and_then(|o| parse_reg_value(&o, name))
}

#[cfg(windows)]
fn set_autostart_named(name: &str, exe: &Path, on: bool) -> Result<(), String> {
    if on {
        reg(&["add", RUN_KEY, "/v", name, "/t", "REG_SZ", "/d", &format!("\"{}\"", exe.display()), "/f"]).map(|_| ())
    } else if autostart_target_named(name).is_some() {
        reg(&["delete", RUN_KEY, "/v", name, "/f"]).map(|_| ())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------- Linux

/// `~/.config/autostart/<name>.desktop`: the desktop starts what is listed there at login (the XDG autostart rule).
#[cfg(target_os = "linux")]
fn desktop_file(name: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join(format!("{}.desktop", name.to_lowercase())))
}

/// The `Exec=` of the entry, without the quotes around a path with spaces.
#[cfg(target_os = "linux")]
fn parse_desktop_exec(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find_map(|l| l.strip_prefix("Exec="))?.trim();
    let inner = line.strip_prefix('"').and_then(|l| l.strip_suffix('"')).unwrap_or(line);
    // inside quotes a backslash makes the next character literal (`\"`, `\\`, `` \` ``, `\$`)
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some(n)) if matches!(n, '"' | '\\' | '`' | '$') => {
                out.push(n);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    Some(out)
}

#[cfg(target_os = "linux")]
fn desktop_entry(exe: &Path) -> String {
    // a path with spaces is quoted; a backslash or a quote inside is escaped, as the desktop-entry rules ask
    let path = exe.display().to_string().replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$");
    format!("[Desktop Entry]\nType=Application\nName=SwapCraft drop bar\nComment=The drop bar and the conversion queue of SwapCraft\nExec=\"{path}\"\nIcon=swapcraft\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n")
}

#[cfg(target_os = "linux")]
fn autostart_target_named(name: &str) -> Option<String> {
    std::fs::read_to_string(desktop_file(name)?).ok().and_then(|t| parse_desktop_exec(&t))
}

#[cfg(target_os = "linux")]
fn set_autostart_named(name: &str, exe: &Path, on: bool) -> Result<(), String> {
    let path = desktop_file(name).ok_or("Cannot find your home folder.")?;
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, desktop_entry(exe)).map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Where the running helper writes its process id (and holds its lock): one copy per person.
#[cfg(unix)]
pub fn pid_file() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir);
    let who = std::env::var("USER").unwrap_or_else(|_| "user".to_owned());
    Some(dir.join(format!("swapcraft-tray-{who}.pid")))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn autostart_target_named(_name: &str) -> Option<String> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
fn set_autostart_named(_name: &str, _exe: &Path, _on: bool) -> Result<(), String> {
    Err("Starting with the computer is not available here.".to_owned())
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

/// The process id the running helper wrote in its pid file, if that process is alive.
#[cfg(unix)]
fn running_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file()?).ok()?.trim().parse().ok()?;
    #[cfg(target_os = "linux")]
    let alive = Path::new(&format!("/proc/{pid}")).exists();
    #[cfg(not(target_os = "linux"))]
    let alive = std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    alive.then_some(pid)
}

pub fn running() -> bool {
    #[cfg(windows)]
    {
        window().is_some()
    }
    #[cfg(unix)]
    {
        running_pid().is_some()
    }
    #[cfg(not(any(windows, unix)))]
    false
}

/// Starts the helper if it is not running.
pub fn start(exe: &Path) -> Result<(), String> {
    if !SUPPORTED {
        return Err("The drop bar is not available here.".to_owned());
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
    // SIGTERM: the helper ends its loop and leaves cleanly
    #[cfg(unix)]
    if let Some(pid) = running_pid() {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
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

    /// The autostart entry on Linux is a `.desktop` file: written, read back (a path with spaces and a quote), removed.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_desktop_entry_is_written_read_and_removed() {
        let home = std::env::temp_dir().join(format!("vf-autostart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        // the entry goes under XDG_CONFIG_HOME, so the test never touches the real one
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &home) };
        let name = "SwapCraftTest";
        let exe = Path::new("/opt/Swap Craft \"x\"/swapcraft-tray");
        assert!(autostart_target_named(name).is_none());
        set_autostart_named(name, exe, true).unwrap();
        assert_eq!(autostart_target_named(name).as_deref(), Some(exe.to_str().unwrap()), "the path with a space and a quote comes back whole");
        assert!(std::fs::read_to_string(desktop_file(name).unwrap()).unwrap().contains("X-GNOME-Autostart-enabled=true"));
        set_autostart_named(name, exe, false).unwrap();
        assert!(autostart_target_named(name).is_none());
        set_autostart_named(name, exe, false).unwrap(); // removing what is not there is fine
        let _ = std::fs::remove_dir_all(&home);
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
