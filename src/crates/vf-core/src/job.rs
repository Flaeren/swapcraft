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
        let ext = p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
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

/// `name.pdf`, or `name (2).pdf` and so on if it exists.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let ext = path.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
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
    let tmp = scratch_dir().map_err(|e| e.to_string())?;
    let result = run_in(&tmp, spec, src, out_dir);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn run_in(tmp: &Path, spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
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
    Ok(out)
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
            let outcome = Outcome { source: item.source, result: (item.run)(&notify) };
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
            Item { label, source: src, run: Box::new(move |_| convert_one(&spec, &source, &out_dir)) }
        })
        .collect();
    spawn_tasks(items, tx, cancel);
}

#[cfg(test)]
mod tests {
    use super::*;

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
