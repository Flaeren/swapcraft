//! Running an engine's `convert <in> <out>` on one file or a batch, off the UI thread.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// One document takes well under a second; this only stops a hung engine.
const TIMEOUT: Duration = Duration::from_secs(600);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What to run. `extra_args` and `env` come from the app's options.
#[derive(Clone, Debug)]
pub struct Spec {
    pub cli: PathBuf,
    pub out_ext: String,
    pub extra_args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub source: PathBuf,
    pub result: Result<PathBuf, String>,
}

pub enum Event {
    Start { index: usize, total: usize, name: String },
    Done { index: usize, outcome: Outcome },
    /// A remark about the item being worked on, shown before its result.
    Note(String),
    Cancelled,
    Finished(Vec<Outcome>),
}

/// Where the outputs go.
#[derive(Clone, Debug)]
pub enum Destination {
    /// Next to each source file.
    Beside,
    Folder(PathBuf),
}

/// Documents among `paths` (files, or folders searched with `recursive`) whose extension is in `exts`
/// (lower case, without the dot). Office lock files (`~$name.docx`) are not documents.
pub fn collect(paths: &[PathBuf], exts: &[&str], recursive: bool) -> Vec<PathBuf> {
    fn is_doc(p: &Path, exts: &[&str]) -> bool {
        // a file with no extension counts by its content (Linux text editors save "notes", not "notes.txt")
        let ext = crate::sniff::ext_or_kind(p);
        let lock = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("~$"));
        p.is_file() && !lock && exts.contains(&ext.as_str())
    }
    fn walk(dir: &Path, exts: &[&str], recursive: bool, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if recursive {
                    walk(&p, exts, recursive, out);
                }
            } else if is_doc(&p, exts) {
                out.push(p);
            }
        }
    }
    let mut found = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, exts, recursive, &mut found);
        } else if is_doc(p, exts) {
            found.push(p.clone());
        }
    }
    found.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    found.dedup_by_key(|p| p.to_string_lossy().to_lowercase());
    found
}

/// The longest file name a folder takes: 255 bytes on Linux (and 255 characters on Windows, counted here in bytes
/// as well, the stricter of the two).
const MAX_NAME: usize = 255;

/// `stem` and `suffix` (an " (2)" and the extension) as one name of at most [`MAX_NAME`] bytes: when it is longer,
/// the stem is cut (on a character boundary) and the suffix kept, so a name near the limit still gets its " (2)"
/// and a long name still keeps its extension.
fn fitted_name(stem: &str, suffix: &str) -> String {
    let room = MAX_NAME.saturating_sub(suffix.len());
    if stem.len() <= room {
        return format!("{stem}{suffix}");
    }
    let mut end = room;
    while end > 0 && !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &stem[..end])
}

/// `name.pdf`, or `name (2).pdf` and so on if it exists. A name too long for the file system is shortened first.
pub fn unique_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let ext = path.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    let first = path.with_file_name(fitted_name(stem, &ext));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| path.with_file_name(fitted_name(stem, &format!(" ({n}){ext}"))))
        .find(|c| !c.exists())
        .expect("an unused name exists")
}

pub fn scratch_dir() -> std::io::Result<PathBuf> {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("voidflare_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn last_line(path: &Path) -> String {
    fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).trim().lines().last().unwrap_or("").to_owned())
        .unwrap_or_default()
}

/// Converts one file. The engine writes to a temp file with a plain name first, so accents and odd
/// spaces in the final name never reach it.
pub fn convert_one(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    convert_one_noted(spec, src, out_dir, &|_| {})
}

/// [`convert_one`] that also reports side notes (documents embedded in a Word file, for one) through `note`.
pub fn convert_one_noted(spec: &Spec, src: &Path, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let tmp = scratch_dir().map_err(|e| e.to_string())?;
    // The engine runs in the scratch folder, so a relative path would be looked up there.
    let src = &std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let result = run_in(&tmp, spec, src, out_dir, note);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn run_in(tmp: &Path, spec: &Spec, src: &Path, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let target = tmp.join(format!("out.{}", spec.out_ext));
    let mut cmd = Command::new(&spec.cli);
    cmd.arg("convert").arg(src).arg(&target).args(&spec.extra_args);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    run_command(&mut cmd, tmp)?;
    if !target.is_file() {
        return Err("the engine finished but wrote no file".to_owned());
    }
    // The engine draws an embedded document as a picture; its pages follow the converted PDF.
    if crate::embedded::applies(spec, src) {
        crate::embedded::append_to_pdf(src, &target, &spec.cli, note);
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    deliver(&target, out_dir, &format!("{stem}.{}", spec.out_ext))
}

/// Moves a finished file into `out_dir` as `name`, or `name (2)` if that exists.
pub fn deliver(file: &Path, out_dir: &Path, name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let out = unique_path(&out_dir.join(name));
    if fs::rename(file, &out).is_err() {
        // Another drive: rename cannot cross volumes.
        fs::copy(file, &out).map_err(|e| e.to_string())?;
    }
    normalize_modes(&out);
    Ok(out)
}

/// The permissions a new file and a new folder get from this process's umask (`None` off Unix).
#[cfg(unix)]
fn new_modes() -> Option<(u32, u32)> {
    use std::os::unix::fs::PermissionsExt;
    static MODES: std::sync::OnceLock<Option<(u32, u32)>> = std::sync::OnceLock::new();
    *MODES.get_or_init(|| {
        let probe = std::env::temp_dir().join(format!("voidflare_umask_{}", std::process::id()));
        let _ = fs::remove_file(&probe);
        let _ = fs::remove_dir(&probe);
        let file = File::create(&probe).ok().and_then(|_| fs::metadata(&probe).ok()).map(|m| m.permissions().mode() & 0o777);
        let _ = fs::remove_file(&probe);
        let dir = fs::create_dir(&probe).ok().and_then(|()| fs::metadata(&probe).ok()).map(|m| m.permissions().mode() & 0o777);
        let _ = fs::remove_dir(&probe);
        file.zip(dir)
    })
}

/// Some engines write their results private to the user (mode 0600, as a temporary file would be): a result is
/// handed over with the permissions any file made by the user would have, so the group, a shared folder and a
/// network share see it as they see the user's other files. A no-op on Windows. Folders and what is in them too.
fn normalize_modes(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Some((file_mode, dir_mode)) = new_modes() else { return };
        fn apply(p: &Path, file_mode: u32, dir_mode: u32) {
            let Ok(meta) = fs::symlink_metadata(p) else { return };
            if meta.is_dir() {
                let _ = fs::set_permissions(p, fs::Permissions::from_mode(dir_mode));
                for entry in fs::read_dir(p).into_iter().flatten().flatten() {
                    apply(&entry.path(), file_mode, dir_mode);
                }
            } else if meta.is_file() {
                let _ = fs::set_permissions(p, fs::Permissions::from_mode(file_mode));
            }
        }
        apply(path, file_mode, dir_mode);
    }
    #[cfg(not(unix))]
    let _ = path;
}

thread_local! {
    /// The Cancel switch of the work this thread runs ([`spawn_tasks`], a recipe): an engine running when it is
    /// pressed is stopped at once, not at the end of its file.
    static CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> = const { std::cell::RefCell::new(None) };
}

/// What a stopped engine returns: never shown, the caller reports "Cancelled" instead.
pub const CANCELLED: &str = "\u{1}cancelled";

/// Ties the engines run on this thread to `flag` (see [`CANCEL`]).
pub fn watch_cancel(flag: &Arc<AtomicBool>) {
    CANCEL.with(|c| *c.borrow_mut() = Some(Arc::clone(flag)));
}

fn cancel_pressed() -> bool {
    CANCEL.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(Ordering::Relaxed)))
}

/// Runs `cmd` with `tmp` as its folder, no console window and a time limit; stdout and stderr go to
/// files in `tmp` so a chatty engine can never fill a pipe. A failure returns the engine's last line.
pub fn run_command(cmd: &mut Command, tmp: &Path) -> Result<(), String> {
    run_command_for(cmd, tmp, TIMEOUT)
}

/// [`run_command`] with a time limit of the caller's choosing (a long video takes far more than ten minutes).
pub fn run_command_for(cmd: &mut Command, tmp: &Path, limit: Duration) -> Result<(), String> {
    let (out_log, err_log) = (tmp.join("stdout.txt"), tmp.join("stderr.txt"));
    cmd.current_dir(tmp);
    cmd.stdin(Stdio::null())
        .stdout(File::create(&out_log).map_err(|e| e.to_string())?)
        .stderr(File::create(&err_log).map_err(|e| e.to_string())?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(crate::tf("job.timeout", &[("m", &(limit.as_secs() / 60).to_string())]));
            }
            None if cancel_pressed() => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CANCELLED.to_owned());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    if !status.success() {
        let mut msg = last_line(&err_log);
        if msg.is_empty() {
            msg = last_line(&out_log);
        }
        if msg.is_empty() {
            msg = format!("exit code {}", status.code().unwrap_or(-1));
        }
        // the engines start their messages with "error: ", and the log already says it failed
        let msg = msg.strip_prefix("error: ").unwrap_or(&msg).to_owned();
        let mut msg: String = msg.chars().take(300).collect();
        if msg.to_lowercase().contains("password") {
            msg.push_str(&crate::t("job.password_hint"));
        }
        return Err(msg);
    }
    Ok(())
}

/// Like [`run_command`], and returns what the program printed on stdout.
pub fn run_capture(cmd: &mut Command, tmp: &Path) -> Result<String, String> {
    run_command(cmd, tmp)?;
    let bytes = fs::read(tmp.join("stdout.txt")).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// One unit of work for [`spawn_tasks`]: what to show in the log, the file it is about, and the work.
pub struct Item {
    pub label: String,
    pub source: PathBuf,
    pub run: Task,
}

/// The work of an [`Item`]. It gets a function to report side notes to the log (`Event::Note`).
pub type Task = Box<dyn FnOnce(&dyn Fn(String)) -> Result<PathBuf, String> + Send>;

/// Runs the items in turn on a new thread, reporting through `tx`. `cancel` stops it between items.
pub fn spawn_tasks(items: Vec<Item>, tx: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        watch_cancel(&cancel);
        let total = items.len();
        let mut all = Vec::new();
        for (i, item) in items.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(Event::Cancelled);
                break;
            }
            let _ = tx.send(Event::Start { index: i, total, name: item.label });
            let notify = |text: String| {
                let _ = tx.send(Event::Note(text));
            };
            let result = (item.run)(&notify);
            // stopped in the middle: not a failure of the file, and nothing after it runs
            if cancel.load(Ordering::Relaxed) && result.is_err() {
                let _ = tx.send(Event::Cancelled);
                break;
            }
            let outcome = Outcome { source: item.source, result };
            all.push(outcome.clone());
            let _ = tx.send(Event::Done { index: i, outcome });
        }
        let _ = tx.send(Event::Finished(all));
    });
}

/// Where `src`'s output goes for a destination choice.
pub fn out_dir_for(dest: &Destination, src: &Path) -> PathBuf {
    match dest {
        Destination::Beside => src.parent().map(Path::to_path_buf).unwrap_or_default(),
        Destination::Folder(f) => f.clone(),
    }
}

/// Converts every file in turn on a new thread, reporting through `tx`.
pub fn spawn_batch(spec: Spec, files: Vec<PathBuf>, dest: Destination, tx: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    let items = files
        .into_iter()
        .map(|src| {
            let label = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (spec, out_dir, source) = (spec.clone(), out_dir_for(&dest, &src), src.clone());
            Item { label, source: src, run: Box::new(move |note| crate::sniff::with_extension(&source, |s| convert_one_noted(&spec, s, &out_dir, note))) }
        })
        .collect();
    spawn_tasks(items, tx, cancel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_stops_a_running_engine_at_once() {
        let flag = Arc::new(AtomicBool::new(false));
        watch_cancel(&flag);
        let tmp = scratch_dir().unwrap();
        // something that would run for half a minute
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        let f = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            f.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        assert_eq!(run_command(&mut cmd, &tmp), Err(CANCELLED.to_owned()));
        assert!(started.elapsed() < Duration::from_secs(3), "stopped in {:?}", started.elapsed());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join("vf_unique_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        assert_eq!(unique_path(&a), a);
        fs::write(&a, b"x").unwrap();
        assert_eq!(unique_path(&a), dir.join("a (2).pdf"));
        fs::write(dir.join("a (2).pdf"), b"x").unwrap();
        assert_eq!(unique_path(&a), dir.join("a (3).pdf"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_result_is_handed_over_with_the_users_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("vf_modes_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("in")).unwrap();
        let made = dir.join("in").join("made.pdf");
        fs::write(&made, b"x").unwrap();
        fs::set_permissions(&made, fs::Permissions::from_mode(0o600)).unwrap();
        let out = deliver(&made, &dir.join("out"), "made.pdf").unwrap();
        let (file_mode, _) = new_modes().unwrap();
        assert_eq!(fs::metadata(&out).unwrap().permissions().mode() & 0o777, file_mode);
        assert_ne!(file_mode, 0o600, "the test needs a umask that lets the group read");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_at_the_limit_still_gets_its_number() {
        // 251 letters and ".txt" make 255 bytes; its PDF twin needs " (2)" on top of that
        let stem = "M".repeat(251);
        assert_eq!(fitted_name(&stem, ".epub").len(), 255, "a longer extension cuts the stem, not the extension");
        assert!(fitted_name(&stem, ".epub").ends_with(".epub"));
        let two = fitted_name(&stem, " (2).pdf");
        assert_eq!(two.len(), 255);
        assert!(two.ends_with(" (2).pdf"));
        // never in the middle of a character: 100 times "\u{e8}" is 200 bytes, "\u{4e2d}" is 3 bytes each
        let accents = "\u{e8}".repeat(130);
        let cut = fitted_name(&accents, " (10).docx");
        assert!(cut.len() <= 255 && cut.ends_with(" (10).docx"));
        assert!(cut.starts_with('\u{e8}'));
        let dir = std::env::temp_dir().join(format!("vf_long_names_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let long = dir.join(format!("{}.pdf", "M".repeat(251)));
        fs::write(&long, b"x").unwrap();
        let next = unique_path(&long);
        fs::write(&next, b"x").expect("the second name is one the file system takes");
        assert_ne!(next, long);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Runs the real WordCraft if it is installed (skipped otherwise): odd names convert, a broken file
    /// reports an error, and nothing is left behind in the temp folder.
    #[test]
    fn real_engine_odd_names_and_broken_file() {
        let Some(cli) = crate::engine::WORDCRAFT.find() else { return };
        let dir = std::env::temp_dir().join("vf_engine_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let rtf = br"{\rtf1\ansi\deff0 Caff\'e8 citt\'e0 \'f9.\par}";
        let odd = dir.join("Verifica contabilit\u{e0} \u{301} .rtf");
        fs::write(&odd, rtf).unwrap();
        let broken = dir.join("rotto.docx");
        fs::write(&broken, b"not a document").unwrap();
        let spec = Spec { cli, out_ext: "pdf".into(), extra_args: vec![], env: vec![] };
        let out = convert_one(&spec, &odd, &dir).expect("odd name converts");
        assert!(out.is_file());
        let again = convert_one(&spec, &odd, &dir).expect("second run");
        assert_ne!(out, again, "collision gets a new name");
        assert!(convert_one(&spec, &broken, &dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_skips_lock_files_and_other_types() {
        let dir = std::env::temp_dir().join("vf_collect_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["a.docx", "B.DOC", "~$a.docx", "c.txt", "sub/d.rtf"] {
            fs::write(dir.join(f), b"x").unwrap();
        }
        let flat = collect(&[dir.clone()], &["docx", "doc", "rtf"], false);
        assert_eq!(flat.len(), 2);
        let deep = collect(&[dir.clone()], &["docx", "doc", "rtf"], true);
        assert_eq!(deep.len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }
}
