//! The conversion queue: one worker thread takes the dropped files one at a time and runs the route the person
//! picked, through `vf_core::routes::run_route` (the same code the hub, the command line and the MCP server use).
//! The result goes beside the source file, never over anything. The window code only reads snapshots and is told
//! through `notify` that something changed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use vf_core::job::{self, CANCELLED, Destination};
use vf_core::recipe::{self, Out, Recipe, Step};
use vf_core::routes::{self, Route};

use crate::offer::MERGE_ID;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Waiting,
    Running,
    /// The file that was written (for a folder result, the folder).
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub name: String,
    /// What it becomes, as the bar showed it (`PDF`).
    pub to: String,
    pub source: PathBuf,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub waiting: usize,
    pub running: usize,
    pub done: usize,
    pub failed: usize,
}

impl Counts {
    pub fn active(&self) -> usize {
        self.waiting + self.running
    }
}

enum Request {
    /// One file, one route.
    One { id: u64, route: &'static Route },
    /// All of `files` into one PDF (convert each to PDF, then merge); `id` is the queue entry that stands for them.
    Merge { id: u64, files: Vec<PathBuf> },
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);
static TX: OnceLock<Sender<Request>> = OnceLock::new();
static NOTIFY: OnceLock<fn()> = OnceLock::new();
static CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn cancel_flag() -> &'static Arc<AtomicBool> {
    CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

fn notify() {
    if let Some(f) = NOTIFY.get() {
        f();
    }
}

fn entries() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    ENTRIES.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_status(id: u64, status: Status) {
    if let Some(e) = entries().iter_mut().find(|e| e.id == id) {
        e.status = status;
    }
}

fn status_of(id: u64) -> Option<Status> {
    entries().iter().find(|e| e.id == id).map(|e| e.status.clone())
}

/// `f` runs (on the worker thread) every time an entry changes; set once, before the first `submit`.
pub fn on_change(f: fn()) {
    let _ = NOTIFY.set(f);
}

/// Queues `files` to become `to_id` (a `routes::route_id`); returns how many were taken. A file the route does
/// not take is queued too, already failed, so the person sees why nothing came out.
pub fn submit(files: &[PathBuf], to_id: &str, to_label: &str) -> usize {
    let tx = TX.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        std::thread::spawn(move || worker(rx));
        tx
    });
    if to_id == MERGE_ID {
        return submit_merged(tx, files, to_label);
    }
    let mut taken = 0;
    for file in files.iter().filter(|f| f.is_file()) {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let route = routes::find_route(&routes::ext_of(file), to_id);
        let status = if route.is_some() { Status::Waiting } else { Status::Failed("Questo formato non è disponibile per il file.".to_owned()) };
        entries().push(Entry { id, name, to: to_label.to_owned(), source: file.clone(), status });
        if let Some(route) = route {
            let _ = tx.send(Request::One { id, route });
        }
        taken += 1;
    }
    if taken > 0 {
        notify();
    }
    taken
}

/// Queues `files` as ONE entry that becomes a single PDF; returns how many files it holds (0 when none is a file).
fn submit_merged(tx: &Sender<Request>, files: &[PathBuf], to_label: &str) -> usize {
    let mut files: Vec<PathBuf> = files.iter().filter(|f| f.is_file()).cloned().collect();
    // the order of the pages is the order of the names, as in a recipe
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let Some(first) = files.first().cloned() else { return 0 };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let first_name = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = if files.len() > 1 { format!("{first_name} +{}", files.len() - 1) } else { first_name };
    let taken = files.len();
    entries().push(Entry { id, name, to: to_label.to_owned(), source: first, status: Status::Waiting });
    let _ = tx.send(Request::Merge { id, files });
    notify();
    taken
}

/// The recipe behind "one PDF": every file to PDF, then the PDFs into one, beside the first file and named after it
/// with the number of the others (`relazione +3.pdf`); nothing is ever overwritten.
fn merge_recipe(first: &Path, count: usize) -> Recipe {
    let stem = first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // (`{` and `}` in a file name would be taken for the recipe's own placeholders)
    let stem = stem.replace('{', "(").replace('}', ")");
    let mut r = Recipe::new("one PDF");
    r.steps = vec![Step::Convert("pdf".into()), Step::Merge];
    r.out = Out::Beside;
    r.file_name = if count > 1 { format!("{stem} +{}", count - 1) } else { stem };
    r.in_menu = false;
    r
}

/// What a finished merge becomes: the merged file, or why there is none. When some files could not be converted the
/// rest are still merged; the entry then says so (and where the file is) instead of looking like a clean success.
fn merge_status(outcomes: &[job::Outcome], files: usize) -> Status {
    let made = outcomes.iter().find_map(|o| o.result.as_ref().ok().cloned());
    let failed: Vec<&String> = outcomes.iter().filter_map(|o| o.result.as_ref().err()).collect();
    match (made, failed.first()) {
        (Some(path), None) => Status::Done(path),
        (Some(path), Some(why)) => Status::Failed(format!("{} su {files} file non uniti ({}): {}", failed.len(), clean(why), path.display())),
        (None, Some(why)) => Status::Failed(clean(why)),
        (None, None) => Status::Cancelled,
    }
}

fn worker(rx: std::sync::mpsc::Receiver<Request>) {
    let flag = cancel_flag().clone();
    job::watch_cancel(&flag);
    while let Ok(req) = rx.recv() {
        let id = match &req {
            Request::One { id, .. } | Request::Merge { id, .. } => *id,
        };
        if status_of(id) != Some(Status::Waiting) {
            continue; // cancelled while it waited
        }
        set_status(id, Status::Running);
        notify();
        let result = match req {
            Request::One { route, .. } => {
                let source = entries().iter().find(|e| e.id == id).map(|e| e.source.clone()).unwrap_or_default();
                let out_dir = job::out_dir_for(&Destination::Beside, &source);
                match routes::run_route(route, &source, &out_dir, &|_| {}) {
                    Ok(path) => Status::Done(path),
                    Err(e) if e == CANCELLED => Status::Cancelled,
                    Err(e) => Status::Failed(clean(&e)),
                }
            }
            Request::Merge { files, .. } => {
                let (tx, _events) = std::sync::mpsc::channel();
                let outcomes = recipe::run(&merge_recipe(&files[0], files.len()), &files, &tx, &flag);
                if flag.load(Ordering::Relaxed) { Status::Cancelled } else { merge_status(&outcomes, files.len()) }
            }
        };
        set_status(id, result);
        if !entries().iter().any(|e| e.status == Status::Waiting) {
            flag.store(false, Ordering::Relaxed);
        }
        notify();
    }
}

/// The engine's message without the line breaks and trailing noise, one short line for the queue card.
fn clean(e: &str) -> String {
    let line = e.lines().map(str::trim).rev().find(|l| !l.is_empty()).unwrap_or(e.trim());
    line.chars().take(160).collect()
}

pub fn snapshot() -> Vec<Entry> {
    entries().clone()
}

pub fn counts() -> Counts {
    let mut c = Counts::default();
    for e in entries().iter() {
        match e.status {
            Status::Waiting => c.waiting += 1,
            Status::Running => c.running += 1,
            Status::Done(_) => c.done += 1,
            Status::Failed(_) => c.failed += 1,
            Status::Cancelled => {}
        }
    }
    c
}

/// Stops what is running and what waits.
pub fn cancel_all() {
    cancel_flag().store(true, Ordering::Relaxed);
    for e in entries().iter_mut().filter(|e| e.status == Status::Waiting) {
        e.status = Status::Cancelled;
    }
    notify();
}

/// Forgets the finished entries (done, failed, cancelled).
pub fn clear_finished() {
    entries().retain(|e| matches!(e.status, Status::Waiting | Status::Running));
    notify();
}

/// Shows a finished entry's result in Explorer (its folder with the file selected).
pub fn reveal(path: &Path) {
    vf_core::platform::reveal(path);
}

/// Sample entries, only to look at the queue card (`--render-queue`).
pub fn push_demo() {
    let mk = |id: u64, name: &str, to: &str, status: Status| Entry { id, name: name.to_owned(), to: to.to_owned(), source: PathBuf::from(name), status };
    let mut e = entries();
    e.push(mk(1, "lettera.docx", "PDF", Status::Done(PathBuf::from("lettera.pdf"))));
    e.push(mk(2, "offerta.eml", "PDF", Status::Failed("The WordCraft engine was not found next to this app or on this PC.".to_owned())));
    e.push(mk(3, "Bilancio 2020.numbers", "Excel (XLSX)", Status::Running));
    e.push(mk(4, "IMG_4021.heic", "JPG", Status::Waiting));
    e.push(mk(5, "una fattura con un nome davvero molto lungo per provare i puntini finali.pdf", "Word (DOCX)", Status::Waiting));
}

/// `--selftest-merge <file>...`: the files as ONE PDF through the real queue (no window), then what it made.
pub fn selftest_merge(files: &[String]) -> Result<(), String> {
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let taken = submit(&paths, MERGE_ID, "PDF unico");
    println!("taken {taken} file(s) as one entry");
    let start = std::time::Instant::now();
    while counts().active() > 0 && start.elapsed().as_secs() < 300 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let all = snapshot();
    for e in &all {
        println!("{} -> {} : {:?}", e.name, e.to, e.status);
    }
    match all.as_slice() {
        [e] if matches!(e.status, Status::Done(_)) => Ok(()),
        _ => Err("the merge did not finish cleanly".to_owned()),
    }
}
