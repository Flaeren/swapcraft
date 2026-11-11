//! What the command line and the MCP server both offer, as JSON values. One place, so the two can never
//! disagree about what voidflare converts.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vf_core::engine::{self, Engine};
use vf_core::routes::{self, ROUTES};

/// Every source extension and what it can become.
pub fn formats() -> Value {
    let list: Vec<Value> = routes::known_exts()
        .into_iter()
        .map(|ext| {
            let to: Vec<Value> = routes::targets(ext)
                .into_iter()
                .map(|i| json!({ "to": routes::route_id(&ROUTES[i]), "label": ROUTES[i].label, "app": ROUTES[i].app }))
                .collect();
            json!({ "from": ext, "to": to })
        })
        .collect();
    json!({ "formats": list })
}

/// Converts one file. `to` is a target id from [`formats`]. The result goes to `out_dir`, or beside the source.
pub fn convert(input: &Path, to: &str, out_dir: Option<&Path>) -> Result<Value, String> {
    if !input.is_file() {
        return Err(format!("not a file: {}", input.display()));
    }
    let ext = routes::ext_of(input);
    let Some(route) = routes::find_route(&ext, to) else {
        let known: Vec<String> = routes::targets(&ext).into_iter().map(|i| routes::route_id(&ROUTES[i])).collect();
        return Err(if ext.is_empty() {
            "this file has no extension, so SwapCraft can't tell what kind of file it is".to_owned()
        } else if known.is_empty() {
            format!("SwapCraft does not convert .{ext} files (try the formats command)")
        } else {
            format!("a .{ext} file can become: {} (not '{to}')", known.join(", "))
        });
    };
    let dir: PathBuf = out_dir.map(Path::to_path_buf).unwrap_or_else(|| input.parent().map(Path::to_path_buf).unwrap_or_default());
    let notes = std::cell::RefCell::new(Vec::<String>::new());
    let out = routes::run_route(route, input, &dir, &|n| notes.borrow_mut().push(n))?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(json!({ "input": input, "output": out, "format": routes::route_id(route), "bytes": size, "notes": notes.into_inner() }))
}

/// Which engines voidflare found on this PC, with their versions.
pub fn engines() -> Value {
    let all: [&Engine; 8] = [
        &engine::WORDCRAFT,
        &engine::GRIDCRAFT,
        &engine::DECKCRAFT,
        &engine::PDFCRAFT,
        &engine::PHOTOCRAFT,
        &engine::SOUNDCRAFT,
        &engine::FILMCRAFT,
        &engine::CADCRAFT,
    ];
    let list: Vec<Value> = all
        .into_iter()
        .map(|e| match e.find() {
            Some(p) => json!({ "engine": e.folder, "found": true, "path": p, "version": engine::version_of(&p) }),
            None => json!({ "engine": e.folder, "found": false }),
        })
        .collect();
    json!({ "engines": list })
}

fn pdfcraft() -> Result<PathBuf, String> {
    engine::PDFCRAFT.find().ok_or_else(|| "The PdfCraft engine was not found next to this app or on this PC.".to_owned())
}

fn beside(input: &Path, out_dir: Option<&Path>) -> PathBuf {
    out_dir.map(Path::to_path_buf).unwrap_or_else(|| input.parent().map(Path::to_path_buf).unwrap_or_default())
}

fn done(output: &Path, what: &str) -> Value {
    json!({ "output": output, "operation": what, "bytes": std::fs::metadata(output).map(|m| m.len()).unwrap_or(0) })
}

pub fn pdf_merge(files: &[PathBuf], name: &str, out_dir: Option<&Path>) -> Result<Value, String> {
    if files.len() < 2 {
        return Err("merging needs at least two PDF files".to_owned());
    }
    let dir = beside(&files[0], out_dir);
    let name = if name.to_lowercase().ends_with(".pdf") { name.to_owned() } else { format!("{name}.pdf") };
    vf_core::pdf::merge(&pdfcraft()?, files, &dir, &name).map(|o| done(&o, "merge"))
}

pub fn pdf_split(input: &Path, every: Option<u32>, before: Option<&str>, out_dir: Option<&Path>) -> Result<Value, String> {
    let by = match (every, before) {
        (Some(n), None) if n > 0 => vf_core::pdf::SplitBy::Every(n),
        (None, Some(list)) if !list.trim().is_empty() => vf_core::pdf::SplitBy::Before(list.to_owned()),
        _ => return Err("give exactly one of 'every' (pages per part) or 'before' (pages where a part starts, e.g. \"3,7\")".to_owned()),
    };
    let folder = vf_core::pdf::split(&pdfcraft()?, input, &by, &beside(input, out_dir))?;
    let parts = std::fs::read_dir(&folder).map(|d| d.count()).unwrap_or(0);
    Ok(json!({ "output": folder, "operation": "split", "parts": parts }))
}

pub fn pdf_extract(input: &Path, pages: &str, out_dir: Option<&Path>) -> Result<Value, String> {
    vf_core::pdf::extract(&pdfcraft()?, input, pages, &beside(input, out_dir)).map(|o| done(&o, "extract"))
}

pub fn pdf_ocr(input: &Path, out_dir: Option<&Path>) -> Result<Value, String> {
    let mut v = vf_core::pdf::batch(&pdfcraft()?, input, vf_core::pdf::Pass::Ocr, &beside(input, out_dir)).map(|o| done(&o, "ocr"))?;
    v["note"] = json!("OCR reads English only: accented letters in other languages come out wrong.");
    Ok(v)
}

pub fn pdf_shrink(input: &Path, ppi: u32, quality: u32, out_dir: Option<&Path>) -> Result<Value, String> {
    vf_core::pdf::batch(&pdfcraft()?, input, vf_core::pdf::Pass::Shrink { ppi, quality }, &beside(input, out_dir)).map(|o| done(&o, "shrink"))
}

/// The saved recipes: name, steps, what they take and where results go.
pub fn recipes() -> Result<Value, String> {
    let list: Vec<Value> = vf_core::recipe::load()?
        .iter()
        .map(|r| {
            let out = match &r.out {
                vf_core::recipe::Out::Beside => json!("beside the source"),
                vf_core::recipe::Out::Folder(f) => json!(f),
            };
            json!({ "name": r.name, "steps": r.steps.iter().map(|s| s.summary()).collect::<Vec<_>>(), "takes": r.takes(), "out": out })
        })
        .collect();
    Ok(json!({ "recipes": list, "file": vf_core::recipe::store_path() }))
}

/// The saved recipe called `name`, or an error listing the names there are.
pub fn find_recipe(name: &str) -> Result<vf_core::recipe::Recipe, String> {
    let all = vf_core::recipe::load()?;
    vf_core::recipe::find(&all, name).cloned().ok_or_else(|| {
        let names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
        if names.is_empty() { "there are no saved recipes (make one in the hub)".to_owned() } else { format!("no recipe called '{name}'; there are: {}", names.join(", ")) }
    })
}

/// Runs a recipe on files (folders are searched, not their subfolders, for the types the recipe takes).
pub fn recipe_run(recipe: &vf_core::recipe::Recipe, inputs: &[PathBuf]) -> Result<Value, String> {
    let takes = recipe.takes();
    let mut files: Vec<PathBuf> = inputs.iter().filter(|p| p.is_file()).cloned().collect();
    let folders: Vec<PathBuf> = inputs.iter().filter(|p| p.is_dir()).cloned().collect();
    files.extend(vf_core::job::collect(&folders, &takes, false));
    if files.is_empty() {
        return Err(format!("no files the recipe '{}' takes ({})", recipe.name, takes.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let outcomes = vf_core::recipe::run(recipe, &files, &tx, &std::sync::atomic::AtomicBool::new(false));
    drop(tx);
    let notes: Vec<String> = rx.into_iter().filter_map(|e| if let vf_core::job::Event::Note(n) = e { Some(n) } else { None }).collect();
    let made: Vec<&PathBuf> = outcomes.iter().filter_map(|o| o.result.as_ref().ok()).collect();
    let failed: Vec<Value> = outcomes.iter().filter_map(|o| o.result.as_ref().err().map(|e| json!({ "input": o.source, "error": e }))).collect();
    let v = json!({ "recipe": recipe.name, "outputs": made, "failed": failed, "notes": notes });
    if made.is_empty() {
        let why: Vec<String> = outcomes.iter().filter_map(|o| o.result.as_ref().err().map(|e| format!("{}: {e}", o.source.display()))).collect();
        return Err(format!("the recipe '{}' made nothing: {}", recipe.name, why.join("; ")));
    }
    Ok(v)
}

/// Size, type and what the file can become. For a PDF also PdfCraft's own summary (pages, title...).
pub fn info(input: &Path) -> Result<Value, String> {
    let meta = std::fs::metadata(input).map_err(|e| format!("{}: {e}", input.display()))?;
    if !meta.is_file() {
        return Err(format!("not a file: {}", input.display()));
    }
    let ext = routes::ext_of(input);
    let can_become: Vec<String> = routes::targets(&ext).into_iter().map(|i| routes::route_id(&ROUTES[i])).collect();
    let mut v = json!({ "path": input, "extension": ext, "bytes": meta.len(), "can_become": can_become });
    if ext == "pdf"
        && let Ok(cli) = pdfcraft()
    {
        let tmp = vf_core::job::scratch_dir().map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(cli);
        cmd.arg("info").arg(input);
        if let Ok(text) = vf_core::job::run_capture(&mut cmd, &tmp)
            && let Ok(parsed) = serde_json::from_str::<Value>(&text)
        {
            v["pdf"] = parsed;
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
    Ok(v)
}
