//! A Start menu shortcut for the hub (on Linux: an entry in the applications menu, a `.desktop` file).
//!
//! Windows has no supported way for a program to pin itself to the taskbar (Microsoft closed it on purpose and the
//! unofficial tricks break between versions), so this does not try: a Start menu entry is a plain `.lnk` file in the
//! user's own Programs folder, needs no administrator rights, and from there the user can pin it with one right click.
//!
//! The `.lnk` is written and read through Windows' own shell-link COM object (`IShellLinkW`), on a short-lived
//! thread of its own so the window's COM state is never touched. PowerShell is not used: on some PCs it takes
//! tens of seconds just to start.

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The shortcut's file name, which is also the name shown in the Start menu. On Linux the menu shows the `Name=`
/// inside the file, and the file name is an id.
const LINK_NAME: &str = if cfg!(windows) { "SwapCraft.lnk" } else { "swapcraft.desktop" };
/// The icon the Linux launcher shows: a PNG shipped beside the hub program.
#[cfg(not(windows))]
pub const ICON_FILE: &str = "swapcraft.png";
/// The name used up to 0.4, before the converter was called SwapCraft. The 0.5 hub replaces it with `SwapCraft.lnk`
/// at its first start (see [`replace_legacy`]); a pin to Start has to be made again.
const LEGACY_LINK_NAME: &str = "voidflare.lnk";

/// The user's Start menu "Programs" folder, where per-user Start menu entries live. Windows is asked for it (the
/// folder can be redirected, as on many office PCs); `%APPDATA%\Microsoft\Windows\Start Menu\Programs` is only the
/// fallback if that fails.
pub fn start_menu_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(p) = com::known_programs_folder() {
            return Some(p);
        }
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Microsoft").join("Windows").join("Start Menu").join("Programs"))
    }
    // Linux: the user's own applications folder, which every desktop's menu reads.
    #[cfg(not(windows))]
    linux::data_home().map(|d| d.join("applications"))
}

/// The path the Start menu shortcut has (or would have) in `dir`: the old `voidflare.lnk` while it exists and
/// no `SwapCraft.lnk` does, else `SwapCraft.lnk`.
pub fn link_path(dir: &Path) -> PathBuf {
    let new = dir.join(LINK_NAME);
    let legacy = dir.join(LEGACY_LINK_NAME);
    if !new.is_file() && legacy.is_file() { legacy } else { new }
}

#[cfg(windows)]
mod com {
    use std::path::PathBuf;
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
    use windows::core::{HSTRING, Interface};

    /// The Programs folder of the Start menu as Windows itself reports it.
    pub fn known_programs_folder() -> Option<PathBuf> {
        known_folder(&FOLDERID_Programs)
    }

    /// The user's desktop as Windows reports it (it can be redirected, to OneDrive for example).
    pub fn known_desktop_folder() -> Option<PathBuf> {
        known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop)
    }

    fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
        // SAFETY: the returned string is owned by COM memory; it is copied and freed right here.
        unsafe {
            let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
            s.filter(|s| !s.is_empty()).map(PathBuf::from)
        }
    }

    /// Runs `f` on a fresh thread with COM initialised on it.
    fn on_com_thread<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        std::thread::spawn(move || {
            // SAFETY: plain COM init/uninit pair on this thread, nothing else uses COM here.
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let ours = hr.is_ok();
            if !ours && hr != RPC_E_CHANGED_MODE {
                return Err(format!("COM could not start: {hr:?}"));
            }
            let r = f();
            if ours {
                unsafe { CoUninitialize() };
            }
            r
        })
        .join()
        .map_err(|_| "the shortcut worker crashed".to_owned())?
    }

    pub fn write(link: PathBuf, target: PathBuf) -> Result<(), String> {
        on_com_thread(move || unsafe {
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("shell link: {e}"))?;
            shell.SetPath(&HSTRING::from(target.as_os_str())).map_err(|e| format!("target: {e}"))?;
            if let Some(dir) = target.parent() {
                shell.SetWorkingDirectory(&HSTRING::from(dir.as_os_str())).map_err(|e| format!("working folder: {e}"))?;
            }
            let _ = shell.SetDescription(&HSTRING::from("Offline file converters"));
            let _ = shell.SetIconLocation(&HSTRING::from(target.as_os_str()), 0);
            let file: IPersistFile = shell.cast().map_err(|e| format!("persist: {e}"))?;
            file.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| format!("could not save the shortcut: {e}"))
        })
    }

    pub fn read(link: PathBuf) -> Option<PathBuf> {
        on_com_thread(move || unsafe {
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("shell link: {e}"))?;
            let file: IPersistFile = shell.cast().map_err(|e| format!("persist: {e}"))?;
            file.Load(&HSTRING::from(link.as_os_str()), STGM_READ).map_err(|e| format!("load: {e}"))?;
            let mut buf = [0u16; 1024];
            let mut find = WIN32_FIND_DATAW::default();
            shell.GetPath(&mut buf, &mut find, 0).map_err(|e| format!("path: {e}"))?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
        })
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
    }
}

#[cfg(not(windows))]
mod linux {
    use std::path::{Path, PathBuf};

    /// `$XDG_DATA_HOME`, normally `~/.local/share`.
    pub fn data_home() -> Option<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".local").join("share")))
    }

    /// The desktop folder: `XDG_DESKTOP_DIR` from `~/.config/user-dirs.dirs` (it is translated, `Scrivania` in
    /// Italian), else `~/Desktop`.
    pub fn desktop() -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?);
        let config = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        let from_dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).ok().and_then(|text| {
            text.lines().find_map(|l| {
                let v = l.trim().strip_prefix("XDG_DESKTOP_DIR=")?.trim().trim_matches('"');
                let v = v.replace("$HOME", &home.to_string_lossy());
                (!v.is_empty()).then(|| PathBuf::from(v))
            })
        });
        Some(from_dirs.unwrap_or_else(|| home.join("Desktop")))
    }

    /// A value for `Exec=`: the path in double quotes, with the characters the spec says to escape.
    fn quote(p: &Path) -> String {
        let mut out = String::from("\"");
        for c in p.to_string_lossy().chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn unquote(v: &str) -> String {
        let v = v.trim();
        let inner = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(v);
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                '%' => {
                    if chars.next() == Some('%') {
                        out.push('%');
                    }
                }
                _ => out.push(c),
            }
        }
        out
    }

    /// The text of the launcher. `%F` lets the menu pass the files dropped on it.
    pub fn entry(target: &Path) -> String {
        let dir = target.parent().unwrap_or(Path::new("/"));
        let icon = dir.join(super::ICON_FILE);
        let mut text = String::from("[Desktop Entry]\nType=Application\nName=SwapCraft\nGenericName=File converter\n");
        text.push_str("Comment=Offline file converters\n");
        text.push_str(&format!("Exec={} %F\n", quote(target)));
        text.push_str(&format!("Path={}\n", dir.display()));
        if icon.is_file() {
            text.push_str(&format!("Icon={}\n", icon.display()));
        }
        // the hub's X11 window class is "voidflare" (eframe's app name, kept like the settings folder): docks match
        // the window to this launcher, and to its icon, by that class
        text.push_str("Terminal=false\nCategories=Utility;\nStartupWMClass=voidflare\n");
        text
    }

    pub fn write(link: &Path, target: &Path, executable: bool) -> Result<(), String> {
        std::fs::write(link, entry(target)).map_err(|e| format!("could not write {}: {e}", link.display()))?;
        if executable {
            // A launcher on the desktop runs only when it is executable; GNOME also wants it marked trusted.
            crate::platform::set_mode(link, 0o755);
            let _ = std::process::Command::new("gio")
                .arg("set")
                .arg(link)
                .arg("metadata::trusted")
                .arg("true")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        Ok(())
    }

    /// The program an existing launcher starts (its `Exec=` without the `%F`).
    pub fn read(link: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(link).ok()?;
        let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?.trim();
        let exec = exec.strip_suffix("%F").unwrap_or(exec).trim();
        Some(PathBuf::from(unquote(exec))).filter(|p| !p.as_os_str().is_empty())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_launcher_points_back_at_its_program() {
            let dir = std::env::temp_dir().join(format!("vf-desktop-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let app = dir.join("Swap Craft $1 100%");
            std::fs::create_dir_all(&app).unwrap();
            let target = app.join("swapcraft");
            std::fs::write(&target, b"ELF").unwrap();
            std::fs::write(app.join(super::super::ICON_FILE), b"png").unwrap();
            let link = dir.join("swapcraft.desktop");
            write(&link, &target, true).unwrap();
            let text = std::fs::read_to_string(&link).unwrap();
            assert!(text.contains("Exec=\"") && text.contains("\\$1 100%%") && text.contains("Icon="), "{text}");
            assert_eq!(read(&link).unwrap(), target);
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&link).unwrap().permissions().mode() & 0o111, 0o111, "a desktop launcher must be executable");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Creates (or replaces) the shortcut to `target` in `dir`; its working folder is the one `target` sits in.
pub fn create_in(dir: &Path, target: &Path) -> Result<(), String> {
    if !target.is_file() {
        return Err(format!("{} does not exist", target.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let link = link_path(dir);
    #[cfg(windows)]
    com::write(link.clone(), target.to_path_buf())?;
    // Linux: the applications menu reads the file as it is; on the desktop it must also be executable.
    #[cfg(not(windows))]
    {
        let on_desktop = desktop_dir().is_some_and(|d| d == dir);
        return linux::write(&link, target, on_desktop);
    }
    #[cfg(windows)]
    if link.is_file() {
        notify_shell(&link, true);
        Ok(())
    } else {
        Err("Windows did not create the shortcut".into())
    }
}

/// Tells Explorer (the desktop, the Start menu) that a shortcut appeared or went away. Without it a plain file
/// delete shows up late or not at all: the icon stays on the desktop and Start lags one change behind.
fn notify_shell(link: &Path, created: bool) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Shell::{SHCNE_CREATE, SHCNE_DELETE, SHCNF_PATHW, SHChangeNotify};
        let wide: Vec<u16> = link.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        SHChangeNotify(if created { SHCNE_CREATE } else { SHCNE_DELETE }, SHCNF_PATHW, Some(wide.as_ptr().cast()), None);
    }
    #[cfg(not(windows))]
    let _ = (link, created);
}

/// What the shortcut in `dir` points to, if there is one.
pub fn target_in(dir: &Path) -> Option<PathBuf> {
    let link = link_path(dir);
    if !link.is_file() {
        return None;
    }
    #[cfg(windows)]
    return com::read(link);
    #[cfg(not(windows))]
    linux::read(&link)
}

/// Deletes the shortcut in `dir`; fine if it was not there.
pub fn remove_in(dir: &Path) -> Result<(), String> {
    let link = link_path(dir);
    match std::fs::remove_file(&link) {
        Ok(()) => {
            notify_shell(&link, false);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove the shortcut: {e}")),
    }
}

/// Start menu shortcut state for the running hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// No shortcut.
    Missing,
    /// A shortcut to this very exe.
    Ok,
    /// A shortcut that points somewhere else (the folder was moved or the exe replaced elsewhere).
    Stale(PathBuf),
}

/// Compares the shortcut in `dir` with `exe`.
pub fn state_in(dir: &Path, exe: &Path) -> State {
    if !link_path(dir).is_file() {
        return State::Missing;
    }
    match target_in(dir) {
        Some(t) if same_file(&t, exe) => State::Ok,
        Some(t) => State::Stale(t),
        None => State::Stale(PathBuf::new()),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// The running hub's exe.
pub fn this_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Replaces an old `voidflare.lnk` in `dir` with `SwapCraft.lnk` to `target`, when the old one points into the folder
/// `target` is in (a copy of the hub elsewhere, a test build, leaves it alone). True when it was replaced.
pub fn replace_legacy_in(dir: &Path, target: &Path) -> Result<bool, String> {
    let legacy = dir.join(LEGACY_LINK_NAME);
    if !legacy.is_file() {
        return Ok(false);
    }
    #[cfg(windows)]
    let here = com::read(legacy.clone()).is_some_and(|t| matches!((t.parent(), target.parent()), (Some(a), Some(b)) if same_file(a, b)));
    #[cfg(not(windows))]
    let here = false;
    if !here {
        return Ok(false);
    }
    // Windows ties a pin to Start to the shortcut's file path (checked with Export-StartLayout: "desktopAppLink"), and
    // ignores a display name from desktop.ini for pins, so the pin cannot survive the new name: the notice asks to pin
    // SwapCraft again. Decided with Gabriele on 9/11/2026: the new name over the pin.
    std::fs::remove_file(&legacy).map_err(|e| format!("could not remove the old shortcut: {e}"))?;
    notify_shell(&legacy, false);
    create_in(dir, target).map(|()| true)
}

/// [`replace_legacy_in`] for the user's Start menu and the running hub.
pub fn replace_legacy() -> Result<bool, String> {
    let dir = start_menu_dir().ok_or("no APPDATA folder")?;
    replace_legacy_in(&dir, &this_exe().ok_or("cannot find the hub's own file")?)
}

/// Where a shortcut to the hub can go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The app list of the Start menu.
    Start,
    /// The user's desktop.
    Desktop,
}

/// The user's desktop folder: asked of Windows, `%USERPROFILE%\Desktop` only as the fallback (Linux: see `linux::desktop`).
pub fn desktop_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(p) = com::known_desktop_folder() {
            return Some(p);
        }
        std::env::var_os("USERPROFILE").map(|d| PathBuf::from(d).join("Desktop"))
    }
    #[cfg(not(windows))]
    linux::desktop()
}

/// The folder of a place.
pub fn dir_of(place: Place) -> Option<PathBuf> {
    match place {
        Place::Start => start_menu_dir(),
        Place::Desktop => desktop_dir(),
    }
}

pub fn create_at(place: Place) -> Result<(), String> {
    let dir = dir_of(place).ok_or("cannot find the folder")?;
    create_in(&dir, &this_exe().ok_or("cannot find the hub's own file")?)
}

pub fn remove_at(place: Place) -> Result<(), String> {
    remove_in(&dir_of(place).ok_or("cannot find the folder")?)
}

pub fn state_at(place: Place) -> State {
    match (dir_of(place), this_exe()) {
        (Some(dir), Some(exe)) => state_in(&dir, &exe),
        _ => State::Missing,
    }
}

pub fn create() -> Result<(), String> {
    create_at(Place::Start)
}

pub fn remove() -> Result<(), String> {
    remove_at(Place::Start)
}

pub fn state() -> State {
    state_at(Place::Start)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vf-shortcut-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn creates_reads_and_removes_a_real_shortcut() {
        let dir = scratch("real");
        // any real exe will do as the target; this test's own binary exists by definition
        let exe = std::env::current_exe().unwrap();
        assert_eq!(state_in(&dir, &exe), State::Missing);
        create_in(&dir, &exe).unwrap();
        assert!(link_path(&dir).is_file());
        assert_eq!(state_in(&dir, &exe), State::Ok);
        // a second call replaces it without complaint
        create_in(&dir, &exe).unwrap();
        remove_in(&dir).unwrap();
        assert_eq!(state_in(&dir, &exe), State::Missing);
        remove_in(&dir).unwrap(); // already gone: still fine
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_voidflare_shortcut_is_kept_and_used() {
        let dir = scratch("legacy");
        let exe = std::env::current_exe().unwrap();
        assert_eq!(link_path(&dir), dir.join("SwapCraft.lnk"), "new installs get the new name");
        create_in(&dir, &exe).unwrap();
        std::fs::rename(dir.join("SwapCraft.lnk"), dir.join("voidflare.lnk")).unwrap();
        assert_eq!(link_path(&dir), dir.join("voidflare.lnk"));
        assert_eq!(state_in(&dir, &exe), State::Ok, "the old shortcut still counts");
        create_in(&dir, &exe).unwrap();
        assert!(!dir.join("SwapCraft.lnk").exists(), "rewritten in place, not duplicated");
        remove_in(&dir).unwrap();
        assert_eq!(state_in(&dir, &exe), State::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_0_5_hub_replaces_the_old_shortcut() {
        let dir = scratch("replace");
        let exe = std::env::current_exe().unwrap();
        assert!(!replace_legacy_in(&dir, &exe).unwrap(), "nothing to replace");
        // an old shortcut to a hub in another folder is not ours to change
        let elsewhere = std::env::temp_dir().join("vf-elsewhere").join("voidflare.exe");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::copy(&exe, &elsewhere).unwrap();
        create_in(&dir, &elsewhere).unwrap();
        std::fs::rename(dir.join("SwapCraft.lnk"), dir.join("voidflare.lnk")).unwrap();
        assert!(!replace_legacy_in(&dir, &exe).unwrap(), "left alone");
        std::fs::remove_file(dir.join("voidflare.lnk")).unwrap();
        let _ = std::fs::remove_dir_all(elsewhere.parent().unwrap());
        create_in(&dir, &exe).unwrap();
        std::fs::rename(dir.join("SwapCraft.lnk"), dir.join("voidflare.lnk")).unwrap();
        // the real case: the old shortcut points at voidflare.exe, which the rename took away, in the hub's folder
        let gone = exe.with_file_name("voidflare-gone.exe");
        std::fs::copy(&exe, &gone).unwrap();
        std::fs::remove_file(dir.join("voidflare.lnk")).unwrap();
        create_in(&dir, &gone).unwrap();
        std::fs::rename(dir.join("SwapCraft.lnk"), dir.join("voidflare.lnk")).unwrap();
        std::fs::remove_file(&gone).unwrap();
        assert!(replace_legacy_in(&dir, &exe).unwrap());
        assert!(!dir.join("voidflare.lnk").exists() && dir.join("SwapCraft.lnk").is_file());
        assert_eq!(state_in(&dir, &exe), State::Ok);
        assert!(!replace_legacy_in(&dir, &exe).unwrap(), "done once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_reports_a_real_programs_folder() {
        let dir = com::known_programs_folder().expect("SHGetKnownFolderPath(Programs)");
        assert!(dir.is_dir(), "{} is not a folder", dir.display());
        assert_eq!(start_menu_dir().unwrap(), dir);
    }

    #[test]
    fn windows_reports_a_real_desktop_folder() {
        let dir = com::known_desktop_folder().expect("SHGetKnownFolderPath(Desktop)");
        assert!(dir.is_dir(), "{} is not a folder", dir.display());
        assert_eq!(dir_of(Place::Desktop).unwrap(), dir);
        assert_eq!(dir_of(Place::Start), start_menu_dir());
    }

    #[test]
    fn a_moved_target_is_reported_stale() {
        let dir = scratch("stale");
        let exe = std::env::current_exe().unwrap();
        create_in(&dir, &exe).unwrap();
        let other = dir.join("other.exe");
        std::fs::copy(&exe, &other).unwrap();
        assert!(matches!(state_in(&dir, &other), State::Stale(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_target_is_refused_and_odd_names_are_harmless() {
        let dir = scratch("odd name 'quoted' & $x é");
        assert!(create_in(&dir, &dir.join("nope.exe")).is_err());
        let exe = std::env::current_exe().unwrap();
        create_in(&dir, &exe).unwrap();
        assert_eq!(state_in(&dir, &exe), State::Ok);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
