//! Recipes: conversions and PDF operations run one after the other on a set of files, saved under a name.
//! "DWG to PDF, merge everything, make it smaller" is one recipe. Every step is something an app already does
//! (a [`route`](crate::routes) or a PdfCraft operation), so a recipe gives the same result as doing the steps by
//! hand, with the in-between files kept in a scratch folder and only the last ones delivered.
//!
//! Saved in `%APPDATA%\voidflare\recipes.json`, shared by the hub, the command line, the MCP server and the
//! right-click menu.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use serde_json::{Value, json};

use crate::engine::PDFCRAFT;
use crate::job::{Event, Outcome, deliver, scratch_dir, unique_path};
use crate::pdf::{self, Pass, SplitBy};
use crate::pdfmerge::merge_with_bookmarks;
use crate::routes::{self, ext_of, find_route, known_exts, output_ext};

/// How much smaller [`Step::Shrink`] makes a PDF: pictures resampled to `ppi` and saved as JPEG at `quality`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Light,
    Balanced,
    Small,
}

impl Level {
    pub const ALL: [Self; 3] = [Self::Light, Self::Balanced, Self::Small];

    pub fn word(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Balanced => "balanced",
            Self::Small => "small",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.word() == s)
    }

    /// `balanced` is the PDF Tools app's default (150 ppi, quality 60).
    fn pass(self) -> Pass {
        match self {
            Self::Light => Pass::Shrink { ppi: 200, quality: 80 },
            Self::Balanced => Pass::Shrink { ppi: 150, quality: 60 },
            Self::Small => Pass::Shrink { ppi: 100, quality: 45 },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A route by its [`routes::route_id`] (`pdf`, `docx`, `pdf-sheets`...), chosen for each file's format.
    Convert(String),
    /// Every PDF of the run into one, a bookmark per file, in the order of the file names.
    Merge,
    Shrink(Level),
    /// Searchable text on scanned pages (PdfCraft reads English).
    Ocr,
    /// A new PDF every N pages; the result is a folder, so nothing can follow.
    Split(u32),
    /// Only these pages (`1,3,5-8`).
    Extract(String),
}

impl Step {
    /// The kinds of step, in the order the hub offers them.
    pub const KINDS: [&'static str; 6] = ["convert", "merge", "shrink", "ocr", "split", "extract"];

    pub fn kind(&self) -> &'static str {
        match self {
            Step::Convert(_) => "convert",
            Step::Merge => "merge",
            Step::Shrink(_) => "shrink",
            Step::Ocr => "ocr",
            Step::Split(_) => "split",
            Step::Extract(_) => "extract",
        }
    }

    /// A new step of this kind with its usual settings.
    pub fn new_of(kind: &str) -> Option<Step> {
        Some(match kind {
            "convert" => Step::Convert("pdf".into()),
            "merge" => Step::Merge,
            "shrink" => Step::Shrink(Level::Balanced),
            "ocr" => Step::Ocr,
            "split" => Step::Split(1),
            "extract" => Step::Extract("1".into()),
            _ => return None,
        })
    }

    /// One line in English, for the command line and the log: `convert to pdf`, `shrink (balanced)`.
    pub fn summary(&self) -> String {
        match self {
            Step::Convert(to) => format!("convert to {to}"),
            Step::Merge => "merge into one PDF".into(),
            Step::Shrink(l) => format!("shrink ({})", l.word()),
            Step::Ocr => "OCR".into(),
            Step::Split(n) => format!("split every {n} page(s)"),
            Step::Extract(p) => format!("extract pages {p}"),
        }
    }

    /// [`Step::summary`] in the user's language, for logs and progress windows (the command line, which loads no
    /// catalog, keeps the English).
    pub fn shown(&self) -> String {
        let (key, args): (&str, Vec<(&str, String)>) = match self {
            Step::Convert(to) => ("recipe.convert", vec![("to", to.to_uppercase())]),
            Step::Merge => ("recipe.merge", vec![]),
            Step::Shrink(l) => ("recipe.shrink", vec![("level", l.word().to_owned())]),
            Step::Ocr => ("recipe.ocr", vec![]),
            Step::Split(n) => ("recipe.split", vec![("n", n.to_string())]),
            Step::Extract(p) => ("recipe.extract", vec![("pages", p.clone())]),
        };
        let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let text = crate::tf(key, &args);
        if text == key { self.summary() } else { text }
    }

    fn to_json(&self) -> Value {
        match self {
            Step::Convert(to) => json!({"do": "convert", "to": to}),
            Step::Shrink(l) => json!({"do": "shrink", "level": l.word()}),
            Step::Split(n) => json!({"do": "split", "every": n}),
            Step::Extract(p) => json!({"do": "extract", "pages": p}),
            other => json!({"do": other.kind()}),
        }
    }

    fn from_json(v: &Value) -> Result<Step, String> {
        let kind = v.get("do").and_then(Value::as_str).ok_or("a step without \"do\"")?;
        let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(match kind {
            "convert" => Step::Convert(text("to").ok_or("convert without \"to\"")?.to_lowercase()),
            "merge" => Step::Merge,
            "shrink" => Step::Shrink(text("level").as_deref().map_or(Some(Level::Balanced), Level::parse).ok_or("unknown shrink level")?),
            "ocr" => Step::Ocr,
            "split" => Step::Split(v.get("every").and_then(Value::as_u64).filter(|&n| n > 0).ok_or("split needs \"every\" > 0")? as u32),
            "extract" => Step::Extract(text("pages").filter(|p| !p.trim().is_empty()).ok_or("extract needs \"pages\"")?),
            other => return Err(format!("unknown step \"{other}\"")),
        })
    }
}

/// Where the results go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// In the folder of the (first) source file.
    Beside,
    Folder(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub name: String,
    pub steps: Vec<Step>,
    pub out: Out,
    /// The name of each result, without the extension. `{name}` is the source file's name (the first file's,
    /// after a merge), `{folder}` its folder's, `{recipe}` this recipe's. Empty means `{name}`.
    pub file_name: String,
    /// Offered in the File Explorer right-click menu.
    pub in_menu: bool,
}

/// What a file is between two steps: a file of some extension, or a folder (pictures, parts) that ends the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    File(String),
    Folder,
}

impl Recipe {
    pub fn new(name: &str) -> Recipe {
        Recipe { name: name.to_owned(), steps: vec![Step::Convert("pdf".into())], out: Out::Beside, file_name: String::new(), in_menu: true }
    }

    /// What a file of `ext` becomes after each step, or why the recipe cannot take it (`step N: ...`).
    pub fn chain(&self, ext: &str) -> Result<Vec<Shape>, String> {
        if self.steps.is_empty() {
            return Err("the recipe has no steps".into());
        }
        let mut shape = Shape::File(ext.to_lowercase());
        let mut shapes = Vec::new();
        for (i, step) in self.steps.iter().enumerate() {
            let fail = |why: String| Err(format!("step {}: {why}", i + 1));
            let Shape::File(cur) = &shape else {
                return fail("the step before writes a folder, so nothing can follow it".into());
            };
            shape = match step {
                Step::Convert(to) => match find_route(cur, to) {
                    Some(route) => output_ext(route).map_or(Shape::Folder, |e| Shape::File(e.to_owned())),
                    None => return fail(format!("a .{cur} file cannot be converted to {to}")),
                },
                _ if cur != "pdf" => return fail(format!("{} works on PDF files, and here the file is .{cur}", step.summary())),
                Step::Split(_) => Shape::Folder,
                _ => Shape::File("pdf".into()),
            };
            shapes.push(shape.clone());
        }
        Ok(shapes)
    }

    pub fn accepts(&self, ext: &str) -> bool {
        self.chain(ext).is_ok()
    }

    /// Every extension the recipe can start from.
    pub fn takes(&self) -> Vec<&'static str> {
        known_exts().into_iter().filter(|e| self.accepts(e)).collect()
    }

    /// The name of a result, from [`Recipe::file_name`], made safe for Windows.
    fn result_name(&self, source: &Path) -> String {
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
        let folder = source.parent().and_then(Path::file_name).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let template = if self.file_name.trim().is_empty() { "{name}" } else { self.file_name.trim() };
        let name = template.replace("{name}", &stem).replace("{folder}", &folder).replace("{recipe}", &self.name);
        let clean: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
        let clean = clean.trim().trim_end_matches('.').to_owned();
        if clean.is_empty() { stem } else { clean }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "name": self.name,
            "steps": self.steps.iter().map(Step::to_json).collect::<Vec<_>>(),
            "file_name": self.file_name,
            "in_menu": self.in_menu,
        });
        if let Out::Folder(f) = &self.out {
            v["out"] = json!(f.to_string_lossy());
        }
        v
    }

    pub fn from_json(v: &Value) -> Result<Recipe, String> {
        let name = v.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or("a recipe without a name")?;
        let steps = v.get("steps").and_then(Value::as_array).ok_or_else(|| format!("{name}: no \"steps\""))?;
        let steps = steps.iter().map(Step::from_json).collect::<Result<Vec<_>, _>>().map_err(|e| format!("{name}: {e}"))?;
        let out = match v.get("out").and_then(Value::as_str) {
            Some(f) if !f.trim().is_empty() => Out::Folder(PathBuf::from(f)),
            _ => Out::Beside,
        };
        Ok(Recipe {
            name: name.to_owned(),
            steps,
            out,
            file_name: v.get("file_name").and_then(Value::as_str).unwrap_or("").to_owned(),
            in_menu: v.get("in_menu").and_then(Value::as_bool).unwrap_or(true),
        })
    }
}

/// `%APPDATA%\voidflare\recipes.json`, or the file named by `VOIDFLARE_RECIPES` (tests, portable setups).
pub fn store_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VOIDFLARE_RECIPES") {
        return Some(PathBuf::from(p));
    }
    crate::platform::data_file("recipes.json")
}

/// The saved recipes. A missing file is no recipes; a broken one is an error, so it is never overwritten blindly.
pub fn load_from(path: &Path) -> Result<Vec<Recipe>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    v.get("recipes").and_then(Value::as_array).ok_or_else(|| format!("{}: no \"recipes\" list", path.display()))?.iter().map(Recipe::from_json).collect()
}

pub fn load() -> Result<Vec<Recipe>, String> {
    store_path().map_or(Ok(Vec::new()), |p| load_from(&p))
}

/// Writes the recipes (to a temporary file first, so a crash never leaves half a file).
pub fn save_to(path: &Path, recipes: &[Recipe]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let v = json!({"version": 1, "recipes": recipes.iter().map(Recipe::to_json).collect::<Vec<_>>()});
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn save(recipes: &[Recipe]) -> Result<(), String> {
    save_to(&store_path().ok_or("no APPDATA folder")?, recipes)
}

/// The recipe called `name` (case does not matter).
pub fn find<'a>(recipes: &'a [Recipe], name: &str) -> Option<&'a Recipe> {
    recipes.iter().find(|r| r.name.eq_ignore_ascii_case(name.trim()))
}

/// Ready-made recipes for a first start, so the tab is not empty.
pub fn examples() -> Vec<Recipe> {
    vec![
        Recipe { name: "Drawings for the client".into(), steps: vec![Step::Convert("pdf-sheets".into()), Step::Merge, Step::Shrink(Level::Balanced)], out: Out::Beside, file_name: "{folder}".into(), in_menu: true },
        Recipe { name: "Scans to searchable PDF".into(), steps: vec![Step::Ocr, Step::Shrink(Level::Balanced)], out: Out::Beside, file_name: "{name} (searchable)".into(), in_menu: true },
        Recipe { name: "Report pack".into(), steps: vec![Step::Convert("pdf".into()), Step::Merge], out: Out::Beside, file_name: "{folder} - report".into(), in_menu: true },
    ]
}

/// One file on its way through the recipe.
struct Track {
    source: PathBuf,
    current: PathBuf,
}

fn pdfcraft() -> Result<PathBuf, String> {
    PDFCRAFT.find().ok_or_else(|| "The PdfCraft engine was not found next to this app or on this PC.".to_owned())
}

/// Runs the recipe on `files` and reports through `tx` like [`crate::job::spawn_tasks`]: a `Start` per step,
/// notes, then a `Done` per result and `Finished`. Files the recipe cannot take are reported as failed at once.
/// `cancel` stops it between two files or two steps.
pub fn run(recipe: &Recipe, files: &[PathBuf], tx: &mpsc::Sender<Event>, cancel: &AtomicBool) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    let mut tracks = Vec::new();
    let mut sorted = files.to_vec();
    sorted.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    for f in sorted {
        let f = std::path::absolute(&f).unwrap_or(f);
        match recipe.chain(&ext_of(&f)) {
            Ok(_) if f.is_file() => tracks.push(Track { source: f.clone(), current: f }),
            Ok(_) => outcomes.push(Outcome { source: f, result: Err("not a file".into()) }),
            Err(e) => outcomes.push(Outcome { source: f, result: Err(e) }),
        }
    }
    let Ok(work) = scratch_dir() else {
        let all: Vec<Outcome> = tracks.into_iter().map(|t| Outcome { source: t.source, result: Err("no scratch folder".into()) }).chain(outcomes).collect();
        let _ = tx.send(Event::Finished(all.clone()));
        return all;
    };
    let total = recipe.steps.len();
    let mut cancelled = false;
    'steps: for (i, step) in recipe.steps.iter().enumerate() {
        if tracks.is_empty() {
            break;
        }
        let _ = tx.send(Event::Start { index: i, total, name: step.shown() });
        let dir = work.join(format!("step{}", i + 1));
        let note = |text: String| {
            let _ = tx.send(Event::Note(text));
        };
        if let Step::Merge = step {
            let parts: Vec<(String, &Path)> = tracks.iter().map(|t| (t.source.file_stem().unwrap_or_default().to_string_lossy().into_owned(), t.current.as_path())).collect();
            let merged = dir.join("merged.pdf");
            let done = fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|()| merge_with_bookmarks(&parts, &merged));
            match done {
                Ok(()) => {
                    let first = tracks.remove(0).source;
                    let others: Vec<PathBuf> = tracks.drain(..).map(|t| t.source).collect();
                    if !others.is_empty() {
                        let n = (others.len() + 1).to_string();
                        let text = crate::tf("recipe.merged", &[("n", &n)]);
                        note(if text == "recipe.merged" { format!("{n} files merged into one PDF") } else { text });
                    }
                    tracks.push(Track { source: first, current: merged });
                }
                Err(e) => {
                    outcomes.extend(tracks.drain(..).map(|t| Outcome { source: t.source, result: Err(format!("merge: {e}")) }));
                }
            }
            continue;
        }
        let mut next = Vec::new();
        for t in tracks.drain(..) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break 'steps;
            }
            match run_step(step, &t.current, &dir, &note) {
                Ok(out) => next.push(Track { source: t.source, current: out }),
                // stopped by Cancel in the middle of the step: not a failure of this file
                Err(e) if e == crate::job::CANCELLED => {}
                Err(e) => outcomes.push(Outcome { source: t.source, result: Err(format!("{}: {e}", step.shown())) }),
            }
        }
        tracks = next;
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
    }
    if cancelled {
        let _ = tx.send(Event::Cancelled);
    } else {
        for t in tracks {
            let result = place(recipe, &t);
            let index = outcomes.len();
            let outcome = Outcome { source: t.source, result };
            let _ = tx.send(Event::Done { index, outcome: outcome.clone() });
            outcomes.push(outcome);
        }
    }
    let _ = fs::remove_dir_all(&work);
    let _ = tx.send(Event::Finished(outcomes.clone()));
    outcomes
}

/// [`run`] on a new thread.
pub fn spawn(recipe: Recipe, files: Vec<PathBuf>, tx: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        crate::job::watch_cancel(&cancel);
        run(&recipe, &files, &tx, &cancel);
    });
}

fn run_step(step: &Step, input: &Path, dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    // Each file gets its own folder, so two results with the same name never meet.
    let dir = unique_path(&dir.join(input.file_stem().unwrap_or_default()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match step {
        Step::Convert(to) => {
            let route = find_route(&ext_of(input), to).ok_or_else(|| format!("cannot convert .{} to {to}", ext_of(input)))?;
            routes::run_route(route, input, &dir, note)
        }
        Step::Shrink(level) => pdf::batch(&pdfcraft()?, input, level.pass(), &dir),
        Step::Ocr => pdf::batch(&pdfcraft()?, input, Pass::Ocr, &dir),
        Step::Split(n) => pdf::split(&pdfcraft()?, input, &SplitBy::Every(*n), &dir),
        Step::Extract(pages) => pdf::extract(&pdfcraft()?, input, pages, &dir),
        Step::Merge => unreachable!("merge works on all the files at once"),
    }
}

/// Moves a finished result (file or folder) to the recipe's destination under its final name.
fn place(recipe: &Recipe, t: &Track) -> Result<PathBuf, String> {
    let out_dir = match &recipe.out {
        Out::Beside => t.source.parent().map(Path::to_path_buf).unwrap_or_default(),
        Out::Folder(f) => f.clone(),
    };
    let name = recipe.result_name(&t.source);
    if t.current.is_dir() {
        fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
        let target = unique_path(&out_dir.join(&name));
        copy_dir(&t.current, &target)?;
        return Ok(target);
    }
    let ext = t.current.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    deliver(&t.current, &out_dir, &format!("{name}.{ext}"))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(entry.file_name()))?;
        } else {
            fs::copy(&p, to.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(steps: Vec<Step>) -> Recipe {
        Recipe { steps, ..Recipe::new("Test") }
    }

    #[test]
    fn the_chain_follows_the_formats() {
        let r = recipe(vec![Step::Convert("pdf-sheets".into()), Step::Merge, Step::Shrink(Level::Balanced)]);
        assert_eq!(r.takes(), vec!["dwg"]);
        assert!(r.chain("DWG").is_ok(), "case does not matter");
        let docs = recipe(vec![Step::Convert("pdf".into()), Step::Ocr]);
        for ext in ["docx", "xlsx", "pptx", "epub", "rtf"] {
            assert!(docs.accepts(ext), "{ext}");
        }
        assert!(!docs.accepts("pdf"), "a pdf is already a pdf: no route");
        assert!(!docs.accepts("png"), "a picture has no pdf route");
        let err = recipe(vec![Step::Convert("docx".into()), Step::Ocr]).chain("pdf").unwrap_err();
        assert!(err.starts_with("step 2:"), "{err}");
        let pages = recipe(vec![Step::Convert("png".into()), Step::Merge]).chain("pdf").unwrap_err();
        assert!(pages.contains("folder"), "{pages}");
        assert_eq!(recipe(vec![Step::Split(2)]).chain("pdf").unwrap(), vec![Shape::Folder]);
        assert!(recipe(vec![]).chain("pdf").is_err());
    }

    #[test]
    fn recipes_survive_a_round_trip_and_bad_files_are_errors() {
        let dir = std::env::temp_dir().join(format!("vf_recipes_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("recipes.json");
        assert_eq!(load_from(&path).unwrap(), vec![], "no file is no recipes");
        let mut all = examples();
        all[1].out = Out::Folder(PathBuf::from(r"D:\Città\out"));
        all.push(recipe(vec![Step::Split(3), Step::Extract("1,3-4".into())]));
        save_to(&path, &all).unwrap();
        assert_eq!(load_from(&path).unwrap(), all);
        fs::write(&path, "{ broken").unwrap();
        assert!(load_from(&path).is_err());
        fs::write(&path, r#"{"recipes":[{"name":"x","steps":[{"do":"fly"}]}]}"#).unwrap();
        assert!(load_from(&path).unwrap_err().contains("unknown step"));
        assert!(find(&all, " drawings FOR the client").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_names_use_the_template_and_stay_legal() {
        let mut r = recipe(vec![Step::Ocr]);
        let src = if cfg!(windows) { Path::new(r"C:\Cantiere Via Roma\Tavola A1.pdf") } else { Path::new("/home/u/Cantiere Via Roma/Tavola A1.pdf") };
        assert_eq!(r.result_name(src), "Tavola A1");
        r.file_name = "{folder} - {recipe}".into();
        assert_eq!(r.result_name(src), "Cantiere Via Roma - Test");
        r.file_name = "a:b?".into();
        assert_eq!(r.result_name(src), "a_b_");
        r.file_name = " . ".into();
        assert_eq!(r.result_name(src), "Tavola A1", "a name with nothing left falls back to the source's");
    }

    #[test]
    fn every_example_takes_something() {
        for r in examples() {
            assert!(!r.takes().is_empty(), "{}", r.name);
        }
    }

    /// Two Word files through convert, merge, shrink, OCR with the real engines; one PNG that the recipe refuses
    /// (skipped when WordCraft or PdfCraft is missing).
    #[test]
    fn real_engines_run_a_whole_recipe() {
        use crate::engine::WORDCRAFT;
        if WORDCRAFT.find().is_none() || PDFCRAFT.find().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vf_recipe_run_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("Cartella città");
        fs::create_dir_all(&src).unwrap();
        let a = src.join("b secondo.rtf");
        let b = src.join("a primo è.rtf");
        fs::write(&a, br"{\rtf1\ansi Secondo documento.\page Pagina due.\par}").unwrap();
        fs::write(&b, br"{\rtf1\ansi Primo documento.\par}").unwrap();
        let png = src.join("foto.png");
        fs::write(&png, b"not really").unwrap();
        let r = Recipe {
            steps: vec![Step::Convert("pdf".into()), Step::Merge, Step::Shrink(Level::Small), Step::Ocr],
            file_name: "{folder} - pack".into(),
            ..Recipe::new("Pack")
        };
        let (tx, rx) = mpsc::channel();
        let outcomes = run(&r, &[a, b, png.clone()], &tx, &AtomicBool::new(false));
        drop(tx);
        let events: Vec<Event> = rx.into_iter().collect();
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Start { .. })).count(), 4, "one start per step");
        assert!(matches!(events.last(), Some(Event::Finished(_))));
        let refused = outcomes.iter().find(|o| o.source == png).unwrap();
        assert!(refused.result.is_err());
        let made: Vec<&PathBuf> = outcomes.iter().filter_map(|o| o.result.as_ref().ok()).collect();
        assert_eq!(made.len(), 1, "{outcomes:?}");
        assert_eq!(made[0].file_name().unwrap().to_string_lossy(), "Cartella città - pack.pdf");
        let doc = lopdf::Document::load(made[0]).unwrap();
        assert_eq!(doc.get_pages().len(), 3, "1 + 2 pages, in file-name order");
        eprintln!("recipe result: {} ({} bytes)", made[0].display(), fs::metadata(made[0]).unwrap().len());
        let leftovers: Vec<_> = fs::read_dir(&src).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 4, "the three sources and one result, nothing in between: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
