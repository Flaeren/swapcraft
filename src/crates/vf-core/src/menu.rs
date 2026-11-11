//! "Convert with voidflare" in the File Explorer right-click menu.
//!
//! Classic shell verbs under `HKCU\Software\Classes\SystemFileAssociations\.<ext>\shell\voidflare`, one cascading
//! entry per file type with the conversions that type has, the recipes that take it and "Open in <app>". Per user,
//! no administrator rights, written in one go with `reg import` (one process instead of hundreds). On Windows 11 the
//! entry sits under "Show more options": the short menu takes only packaged, signed apps.
//!
//! Each command starts `voidflare.exe --run <action> <file>`. Explorer starts one process per selected file, so
//! [`gather`] lets the first one collect the others' files and run them together.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::recipe::Recipe;
use crate::routes::{self, ROUTES};

/// The drag-and-drop menu's file format, DLL name and class id (shared with the DLL itself).
pub use voidflare_dragdrop::config as dragmenu;

const ROOT: &str = r"HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations";
const VERB: &str = "voidflare";

/// What a menu entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A conversion by [`routes::route_id`].
    Route(String),
    /// A saved recipe by name.
    Recipe(String),
    /// Open the files in an app, by app id.
    Open(String),
}

impl Action {
    pub fn encode(&self) -> String {
        match self {
            Action::Route(id) => format!("route:{id}"),
            Action::Recipe(name) => format!("recipe:{name}"),
            Action::Open(app) => format!("app:{app}"),
        }
    }

    pub fn parse(s: &str) -> Option<Action> {
        let (kind, rest) = s.split_once(':')?;
        if rest.is_empty() {
            return None;
        }
        Some(match kind {
            "route" => Action::Route(rest.to_owned()),
            "recipe" => Action::Recipe(rest.to_owned()),
            "app" => Action::Open(rest.to_owned()),
            _ => return None,
        })
    }
}

/// The menu's words, in the user's language.
pub struct Texts {
    /// "Convert with voidflare"
    pub title: String,
    /// "Recipe: {name}"
    pub recipe: String,
    /// "Open in {app}…"
    pub open_in: String,
    /// "Convert here with voidflare": the drag-and-drop menu (right mouse button, dropped on a folder)
    pub here: String,
}

/// One entry of a file type's submenu.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub label: String,
    pub action: Action,
    /// A line above it (the first recipe, the "Open in" entry).
    pub separator: bool,
}

/// The entries for files of `ext`: its conversions, the recipes that take it (those marked for the menu), and the
/// app with more options. `app_name` gives an app's display name from its id, `None` when it is not installed.
pub fn entries(ext: &str, recipes: &[Recipe], texts: &Texts, app_name: &dyn Fn(&str) -> Option<String>) -> Vec<Entry> {
    let mut out: Vec<Entry> = routes::targets(ext)
        .into_iter()
        .map(|i| Entry { label: routes::shown_label(ROUTES[i].label), action: Action::Route(routes::route_id(&ROUTES[i])), separator: false })
        .collect();
    let mut first = true;
    for r in recipes.iter().filter(|r| r.in_menu && r.accepts(ext)) {
        out.push(Entry { label: texts.recipe.replace("{name}", &r.name), action: Action::Recipe(r.name.clone()), separator: first });
        first = false;
    }
    let mut apps: Vec<&str> = routes::targets(ext).into_iter().map(|i| ROUTES[i].app).collect();
    apps.dedup();
    let mut sep = true;
    for app in apps {
        if let Some(name) = app_name(app) {
            out.push(Entry { label: texts.open_in.replace("{app}", &name), action: Action::Open(app.to_owned()), separator: sep });
            sep = false;
        }
    }
    out
}

/// A string as a `.reg` value: quoted, with `\` and `"` escaped.
fn reg_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// The `.reg` text that adds the menu for every type voidflare converts, starting `exe`.
pub fn reg_add(exe: &Path, recipes: &[Recipe], texts: &Texts, app_name: &dyn Fn(&str) -> Option<String>) -> (String, Vec<&'static str>) {
    let exe_s = exe.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
    let mut exts = Vec::new();
    for ext in routes::known_exts() {
        let items = entries(ext, recipes, texts, app_name);
        if items.is_empty() {
            continue;
        }
        exts.push(ext);
        let key = format!(r"{ROOT}\.{ext}\shell\{VERB}");
        // A clean slate first, so entries of recipes that were deleted or renamed do not linger.
        text += &format!("\r\n[-{key}]\r\n\r\n[{key}]\r\n\"MUIVerb\"={}\r\n\"Icon\"={}\r\n\"SubCommands\"=\"\"\r\n", reg_str(&texts.title), reg_str(&format!("{exe_s},0")));
        for (n, item) in items.iter().enumerate() {
            let sub = format!(r"{key}\shell\{:02}", n + 1);
            text += &format!("\r\n[{sub}]\r\n\"MUIVerb\"={}\r\n\"MultiSelectModel\"=\"Player\"\r\n", reg_str(&item.label));
            if item.separator {
                // ECF_SEPARATORBEFORE
                text += "\"CommandFlags\"=dword:00000020\r\n";
            }
            let command = format!("\"{exe_s}\" --run \"{}\" \"%1\"", item.action.encode());
            text += &format!("\r\n[{sub}\\command]\r\n@={}\r\n", reg_str(&command));
        }
    }
    (text, exts)
}

const CLASSES: &str = r"HKEY_CURRENT_USER\Software\Classes";
/// Where the drag-and-drop handler is registered: dropping on a folder, and on a drive.
const DROP_TARGETS: [&str; 2] = ["Directory", "Drive"];

/// The `.reg` text that registers the drag-and-drop DLL (per user: COM looks in `HKCU\Software\Classes` first).
pub fn drag_reg_add(dll: &Path) -> String {
    let dll_s = dll.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    let id = voidflare_dragdrop::config::CLSID_STR;
    let mut text = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n[{CLASSES}\\CLSID\\{id}]\r\n@=\"voidflare drag-and-drop menu\"\r\n\r\n[{CLASSES}\\CLSID\\{id}\\InprocServer32]\r\n@={}\r\n\"ThreadingModel\"=\"Apartment\"\r\n",
        reg_str(&dll_s)
    );
    for t in DROP_TARGETS {
        text += &format!("\r\n[{CLASSES}\\{t}\\shellex\\DragDropHandlers\\{VERB}]\r\n@=\"{id}\"\r\n");
    }
    text
}

/// The `.reg` text that takes the drag-and-drop DLL's registration away.
pub fn drag_reg_remove() -> String {
    let mut text = format!("Windows Registry Editor Version 5.00\r\n\r\n[-{CLASSES}\\CLSID\\{}]\r\n", voidflare_dragdrop::config::CLSID_STR);
    for t in DROP_TARGETS {
        text += &format!("\r\n[-{CLASSES}\\{t}\\shellex\\DragDropHandlers\\{VERB}]\r\n");
    }
    text
}

/// What the drag-and-drop menu offers for each type: the same entries as the right-click menu, under `texts.here`.
pub fn drag_config(exe: &Path, recipes: &[Recipe], texts: &Texts, app_name: &dyn Fn(&str) -> Option<String>) -> voidflare_dragdrop::config::Config {
    use voidflare_dragdrop::config::{Config, Entry as DragEntry};
    let types = routes::known_exts()
        .into_iter()
        .map(|ext| {
            let list: Vec<DragEntry> = entries(ext, recipes, texts, app_name).into_iter().map(|e| DragEntry { action: e.action.encode(), separator: e.separator, label: e.label }).collect();
            (ext.to_owned(), list)
        })
        .filter(|(_, l)| !l.is_empty())
        .collect();
    Config { exe: PathBuf::from(exe.to_string_lossy().trim_start_matches(r"\\?\")), title: texts.here.clone(), types }
}

fn drag_file() -> Option<PathBuf> {
    voidflare_dragdrop::config::path()
}

/// Registers the DLL next to `exe` and writes its menu file; without the DLL (a build that has none) any older
/// registration is taken away, so Explorer never looks for a DLL that is not there.
fn install_drag(exe: &Path, recipes: &[Recipe], texts: &Texts, app_name: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
    let dll = exe.parent().map(|d| d.join(voidflare_dragdrop::config::DLL_NAME));
    match dll.filter(|d| d.is_file()) {
        Some(dll) => {
            if let Some(p) = drag_file() {
                if let Some(dir) = p.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                fs::write(&p, drag_config(exe, recipes, texts, app_name).write()).map_err(|e| e.to_string())?;
            }
            import(&drag_reg_add(&dll))
        }
        None => uninstall_drag(),
    }
}

fn uninstall_drag() -> Result<(), String> {
    import(&drag_reg_remove())?;
    // `Drive` (and `shellex`, `DragDropHandlers`) may exist only because of us: remove them when empty.
    for t in DROP_TARGETS {
        let base = format!(r"Software\Classes\{t}");
        if delete_if_empty(&format!(r"{base}\shellex\DragDropHandlers")) && delete_if_empty(&format!(r"{base}\shellex")) {
            delete_if_empty(&base);
        }
    }
    if let Some(p) = drag_file() {
        let _ = fs::remove_file(p);
    }
    Ok(())
}

/// The `.reg` text that removes the menu from these types.
pub fn reg_remove(exts: &[String]) -> String {
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
    for ext in exts {
        text += &format!("\r\n[-{ROOT}\\.{ext}\\shell\\{VERB}]\r\n");
    }
    text
}

/// `%APPDATA%\voidflare\menu.txt`: the types the menu was added to, one per line, so removing it later also clears
/// types a newer version no longer converts.
fn state_file() -> Option<PathBuf> {
    crate::platform::data_file("menu.txt")
}

/// Whether this system has the menu at all: File Explorer on Windows; on Linux, Nemo (Linux Mint's file manager),
/// through its "actions". Other Linux file managers have their own ways, not done.
pub const SUPPORTED: bool = cfg!(windows) || cfg!(target_os = "linux");

/// The second Nemo action: "Convert here with SwapCraft" on the empty space of a folder. It converts the files
/// copied with Ctrl+C into that folder: the nearest thing to Windows' drag with the right mouse button, which
/// Nemo has no way to extend.
pub fn nemo_here_path() -> Option<PathBuf> {
    nemo_action_path().map(|p| p.with_file_name("swapcraft-here.nemo_action"))
}

pub fn nemo_here_action(exe: &Path, title: &str) -> String {
    let icon = exe.with_file_name("swapcraft.png");
    let english = crate::i18n::t_in(crate::i18n::Lang::En, "menu.here").unwrap_or_else(|| title.to_owned());
    let mut text = String::from("[Nemo Action]\n");
    text += &format!("Name={}\n", english.replace('\n', " "));
    for lang in crate::i18n::Lang::ALL {
        if let Some(name) = crate::i18n::t_in(lang, "menu.here") {
            text += &format!("Name[{0}]={1}\nComment[{0}]={1}\n", lang.code(), name.replace('\n', " "));
        }
    }
    text += &format!("Comment={}\n", english.replace('\n', " "));
    text += &format!("Exec=\"{}\" --here %P\n", exe.display());
    if icon.is_file() {
        text += &format!("Icon-Name={}\n", icon.display());
    }
    // a click on the empty space of the folder, not on a file
    text += "Selection=none\nExtensions=any;\nQuote=double\n";
    text
}

/// Whether the menu starts this program: false when it was added by a copy elsewhere. Windows: not known (the
/// registry entries are many), so true.
pub fn points_here(exe: &Path) -> bool {
    if cfg!(windows) {
        return true;
    }
    nemo_action_path().and_then(|p| fs::read_to_string(p).ok()).is_some_and(|t| t.contains(&format!("Exec=\"{}\"", exe.display())))
}

/// Nemo's action file: `$XDG_DATA_HOME/nemo/actions/swapcraft.nemo_action` (normally `~/.local/share/...`).
pub fn nemo_action_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".local").join("share")))?;
    Some(base.join("nemo").join("actions").join("swapcraft.nemo_action"))
}

/// The Nemo action: one entry, "Convert with SwapCraft", on files of every type SwapCraft converts; it starts the
/// hub with `--choose` and the selected files, which opens the small window to pick the format, a recipe or an
/// app. (One entry per format would put ten lines in the menu of a PDF: Nemo actions have no submenus.)
pub fn nemo_action(exe: &Path, title: &str, exts: &[&str]) -> String {
    let icon = exe.with_file_name("swapcraft.png");
    let title = title.replace('\n', " ");
    let mut text = String::from("[Nemo Action]\n");
    // English by default and the entry in each of SwapCraft's languages, which Nemo picks by the desktop's
    // language (`Name[it]=...`); `title` (the current language's) when the catalog is not loaded
    let english = crate::i18n::t_in(crate::i18n::Lang::En, "menu.title").unwrap_or_else(|| title.clone());
    text += &format!("Name={}\n", english.replace('\n', " "));
    for lang in crate::i18n::Lang::ALL {
        if let Some(name) = crate::i18n::t_in(lang, "menu.title") {
            text += &format!("Name[{0}]={1}\nComment[{0}]={1}\n", lang.code(), name.replace('\n', " "));
        }
    }
    text += &format!("Comment={title}\n");
    // the program's path in double quotes (it may hold spaces); each file quoted by Nemo itself
    text += &format!("Exec=\"{}\" --choose %F\n", exe.display());
    if icon.is_file() {
        text += &format!("Icon-Name={}\n", icon.display());
    }
    text += "Selection=notnone\n";
    // `none`: files with no extension too (Linux editors save "notes"); the window tells them by their content
    text += &format!("Extensions={};none;\n", exts.join(";"));
    text += "Quote=double\n";
    text
}

/// Whether the menu was added (and not removed since).
pub fn installed() -> bool {
    if !cfg!(windows) {
        return nemo_action_path().is_some_and(|p| p.is_file());
    }
    SUPPORTED
        && state_file().and_then(|p| fs::read_to_string(p).ok()).is_some_and(|t| t.lines().any(|l| !l.trim().is_empty()))
}

fn remembered_exts() -> Vec<String> {
    state_file().and_then(|p| fs::read_to_string(p).ok()).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()).unwrap_or_default()
}

/// Imports `.reg` text: UTF-16 with a byte order mark, as `reg import` expects.
fn import(text: &str) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("voidflare-menu-{}.reg", std::process::id()));
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new("reg");
    cmd.arg("import").arg(&path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| e.to_string());
    let _ = fs::remove_file(&path);
    let out = out?;
    if out.status.success() {
        notify_shell();
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Tells Explorer that file type menus changed, so the new menu shows without logging off.
fn notify_shell() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

/// Adds (or rewrites) the menu for every type, pointing at `exe`.
pub fn install(exe: &Path, recipes: &[Recipe], texts: &Texts, app_name: &dyn Fn(&str) -> Option<String>) -> Result<usize, String> {
    if !SUPPORTED {
        return Err("The right-click menu exists only in Windows File Explorer.".to_owned());
    }
    if !cfg!(windows) {
        let _ = (recipes, app_name);
        let exts: Vec<&str> = routes::known_exts();
        let path = nemo_action_path().ok_or("Cannot find your home folder.")?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&path, nemo_action(exe, &texts.title, &exts)).map_err(|e| e.to_string())?;
        if let Some(here) = nemo_here_path() {
            fs::write(here, nemo_here_action(exe, &texts.here)).map_err(|e| e.to_string())?;
        }
        if let Some(p) = state_file() {
            if let Some(dir) = p.parent() {
                let _ = fs::create_dir_all(dir);
            }
            fs::write(p, exts.join("\n")).map_err(|e| e.to_string())?;
        }
        return Ok(exts.len());
    }
    // Types from an older version that this one no longer converts go first.
    let now: Vec<String> = routes::known_exts().into_iter().map(str::to_owned).collect();
    let stale: Vec<String> = remembered_exts().into_iter().filter(|e| !now.contains(e)).collect();
    if !stale.is_empty() {
        import(&reg_remove(&stale))?;
    }
    let (text, exts) = reg_add(exe, recipes, texts, app_name);
    import(&text)?;
    install_drag(exe, recipes, texts, app_name)?;
    if let Some(p) = state_file() {
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(p, exts.join("\n")).map_err(|e| e.to_string())?;
    }
    Ok(exts.len())
}

/// Removes the menu from every type it was added to.
pub fn uninstall() -> Result<(), String> {
    if !cfg!(windows) {
        for p in [nemo_action_path(), nemo_here_path()].into_iter().flatten() {
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if let Some(p) = state_file() {
            let _ = fs::remove_file(p);
        }
        return Ok(());
    }
    let mut exts = remembered_exts();
    for e in routes::known_exts() {
        if !exts.iter().any(|x| x == e) {
            exts.push(e.to_owned());
        }
    }
    import(&reg_remove(&exts))?;
    uninstall_drag()?;
    // Adding the menu created `.<ext>` and `.<ext>\shell` where there were none; take them away again when nothing
    // else lives in them (another program's entries or values are left alone).
    for ext in &exts {
        let base = format!(r"Software\Classes\SystemFileAssociations\.{ext}");
        if delete_if_empty(&format!(r"{base}\shell")) {
            delete_if_empty(&base);
        }
    }
    if let Some(p) = state_file() {
        let _ = fs::remove_file(p);
    }
    Ok(())
}

/// Deletes `HKCU\<path>` only if it has no subkeys and no values. True when it is gone.
#[cfg(windows)]
fn delete_if_empty(path: &str) -> bool {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegDeleteKeyW, RegOpenKeyExW, RegQueryInfoKeyW};
    use windows::core::HSTRING;
    let name = HSTRING::from(path);
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &name, None, KEY_READ, &mut key).0 != 0 {
            return false;
        }
        let (mut subkeys, mut values) = (0u32, 0u32);
        let read = RegQueryInfoKeyW(key, None, None, None, Some(&mut subkeys), None, None, Some(&mut values), None, None, None, None);
        let _ = RegCloseKey(key);
        read.0 == 0 && subkeys == 0 && values == 0 && RegDeleteKeyW(HKEY_CURRENT_USER, &name).0 == 0
    }
}

#[cfg(not(windows))]
fn delete_if_empty(_path: &str) -> bool {
    false
}

/// FNV-1a, for a folder name that is the same for every process of one click.
fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The files of one right-click, together. Explorer starts one process per selected file: the first to claim the
/// action's inbox (`%TEMP%\voidflare-menu\<hash>`) waits until no more files arrive for a moment and returns them
/// all (its own first); every other process drops its file in the inbox and gets `None`, meaning "exit".
pub fn gather(action: &str, file: &Path) -> Option<Vec<PathBuf>> {
    gather_in(&std::env::temp_dir().join("voidflare-menu"), action, file, Duration::from_millis(1000))
}

pub fn gather_in(base: &Path, action: &str, file: &Path, quiet: Duration) -> Option<Vec<PathBuf>> {
    let inbox = base.join(short_hash(action));
    let me = format!("{}-{}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    for _ in 0..50 {
        let _ = fs::create_dir_all(&inbox);
        let lock = inbox.join("lock");
        // A leader that crashed leaves its lock: after two minutes it no longer counts.
        if let Ok(meta) = fs::metadata(&lock)
            && meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(120))
        {
            let _ = fs::remove_dir_all(&inbox);
            continue;
        }
        let claimed = fs::OpenOptions::new().write(true).create_new(true).open(&lock).map(drop);
        // (the lock file is closed now: Windows will not rename a folder with an open file in it)
        match claimed {
            Ok(()) => return Some(lead(&inbox, file, quiet)),
            Err(_) => {
                // Someone leads: hand the file over. If the inbox was taken meanwhile, the write fails; try again.
                if fs::write(inbox.join(format!("{me}.txt")), file.to_string_lossy().as_bytes()).is_ok() {
                    return None;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Some(vec![file.to_path_buf()])
}

fn lead(inbox: &Path, file: &Path, quiet: Duration) -> Vec<PathBuf> {
    let count = || fs::read_dir(inbox).map(|d| d.count()).unwrap_or(0);
    let (mut seen, mut since, start) = (count(), Instant::now(), Instant::now());
    while since.elapsed() < quiet && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(50));
        let n = count();
        if n != seen {
            (seen, since) = (n, Instant::now());
        }
    }
    // Taking the inbox is one rename: a file that arrives after it fails to land and its process leads a new run.
    let taken = inbox.with_extension(format!("taken{}", std::process::id()));
    let mut files = vec![file.to_path_buf()];
    // A follower may be writing its file this very moment: try a few times.
    let renamed = (0..10).any(|_| fs::rename(inbox, &taken).is_ok() || {
        std::thread::sleep(Duration::from_millis(30));
        false
    });
    if renamed {
        let mut more: Vec<PathBuf> = fs::read_dir(&taken)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .filter_map(|e| fs::read_to_string(e.path()).ok())
            .map(PathBuf::from)
            .collect();
        more.sort();
        files.extend(more);
        let _ = fs::remove_dir_all(&taken);
    }
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::examples;

    fn texts() -> Texts {
        Texts { title: "Convert with voidflare".into(), recipe: "Recipe: {name}".into(), open_in: "Open in {app}…".into(), here: "Convert here with voidflare".into() }
    }

    #[test]
    fn actions_round_trip() {
        for a in [Action::Route("pdf-sheets".into()), Action::Recipe("Drawings: client".into()), Action::Open("dwg-to-pdf".into())] {
            assert_eq!(Action::parse(&a.encode()), Some(a));
        }
        assert_eq!(Action::parse("route:"), None);
        assert_eq!(Action::parse("fly:x"), None);
    }

    #[test]
    fn a_drawing_gets_its_conversions_its_recipe_and_its_app() {
        let name = |id: &str| (id == "dwg-to-pdf").then(|| "DWG to PDF".to_owned());
        let e = entries("dwg", &examples(), &texts(), &name);
        let labels: Vec<&str> = e.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, ["PDF · sheets, merged", "PDF · whole drawing", "Recipe: Drawings for the client", "Open in DWG to PDF…"]);
        assert!(e[2].separator && e[3].separator && !e[1].separator);
        let mut hidden = examples();
        hidden[0].in_menu = false;
        assert_eq!(entries("dwg", &hidden, &texts(), &name).len(), 3, "a recipe can stay out of the menu");
    }

    #[test]
    fn the_reg_text_escapes_paths_and_quotes() {
        let exe = Path::new(r"D:\Programmi città\voidflare\voidflare.exe");
        let (text, exts) = reg_add(exe, &examples(), &texts(), &|_| None);
        assert!(exts.contains(&"dwg") && exts.contains(&"pdf"));
        assert!(text.starts_with("Windows Registry Editor Version 5.00\r\n"));
        assert!(text.contains(r#"[HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.dwg\shell\voidflare\shell\01\command]"#));
        assert!(text.contains(r#"@="\"D:\\Programmi città\\voidflare\\voidflare.exe\" --run \"route:pdf-sheets\" \"%1\"""#), "{}", &text[..600]);
        assert!(text.contains(r#"[-HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.dwg\shell\voidflare]"#));
        assert!(reg_remove(&["pdf".into()]).contains(r"[-HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.pdf\shell\voidflare]"));
    }

    #[test]
    fn the_drag_and_drop_dll_is_registered_per_user_on_folders_and_drives() {
        let dll = Path::new(r"D:\Programmi città\voidflare\voidflare_dragdrop.dll");
        let text = drag_reg_add(dll);
        let id = dragmenu::CLSID_STR;
        assert!(text.starts_with("Windows Registry Editor Version 5.00\r\n"));
        assert!(text.contains(&format!("[HKEY_CURRENT_USER\\Software\\Classes\\CLSID\\{id}\\InprocServer32]\r\n@=\"D:\\\\Programmi città\\\\voidflare\\\\voidflare_dragdrop.dll\"\r\n\"ThreadingModel\"=\"Apartment\"")), "{text}");
        for t in ["Directory", "Drive"] {
            assert!(text.contains(&format!("[HKEY_CURRENT_USER\\Software\\Classes\\{t}\\shellex\\DragDropHandlers\\voidflare]\r\n@=\"{id}\"")), "{text}");
        }
        assert!(!text.contains("HKEY_LOCAL_MACHINE"));
        let off = drag_reg_remove();
        assert!(off.contains(&format!("[-HKEY_CURRENT_USER\\Software\\Classes\\CLSID\\{id}]")));
        assert!(off.contains(r"[-HKEY_CURRENT_USER\Software\Classes\Drive\shellex\DragDropHandlers\voidflare]"));
    }

    #[test]
    fn the_drag_menu_file_has_the_right_click_entries_under_its_own_title() {
        let name = |id: &str| (id == "dwg-to-pdf").then(|| "DWG to PDF".to_owned());
        let cfg = drag_config(Path::new(r"\\?\C:\v\voidflare.exe"), &examples(), &texts(), &name);
        assert_eq!(cfg.exe, PathBuf::from(r"C:\v\voidflare.exe"), "no \\\\?\\ prefix");
        assert_eq!(cfg.title, "Convert here with voidflare");
        let back = dragmenu::Config::parse(&cfg.write()).unwrap();
        let dwg: Vec<String> = back.entries_for(&[PathBuf::from("a.dwg")]).into_iter().map(|e| e.label).collect();
        let right_click: Vec<String> = entries("dwg", &examples(), &texts(), &name).into_iter().map(|e| e.label).collect();
        assert_eq!(dwg, right_click);
        assert!(back.entries_for(&[PathBuf::from("a.zip")]).is_empty());
    }

    #[test]
    fn processes_of_one_click_are_gathered_by_the_first() {
        let base = std::env::temp_dir().join(format!("vf_gather_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let base = base.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(i * 30));
                    gather_in(&base, "route:pdf", Path::new(&format!(r"C:\x\file{i}.docx")), Duration::from_millis(400))
                })
            })
            .collect();
        let results: Vec<Option<Vec<PathBuf>>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let leaders: Vec<&Vec<PathBuf>> = results.iter().flatten().collect();
        assert_eq!(leaders.len(), 1, "{results:?}");
        assert_eq!(leaders[0].len(), 6);
        assert_eq!(leaders[0][0], PathBuf::from(r"C:\x\file0.docx"), "the leader's own file first");
        // A second click later is a new run.
        assert_eq!(gather_in(&base, "route:pdf", Path::new(r"C:\y.docx"), Duration::from_millis(100)), Some(vec![PathBuf::from(r"C:\y.docx")]));
        let _ = fs::remove_dir_all(&base);
    }
}
