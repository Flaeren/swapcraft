//! Engine compatibility matrix.
//!
//! For each ArtCraft engine voidflare drives, runs one small real conversion with (a) the copy we pin in
//! `src/engines/<name>/` and (b) the copy installed on this PC, then prints which versions passed. A pack may
//! reuse the PC's engine only if its exact version is listed in `src/engine-compat.json`; that file is written
//! by hand from this matrix, and this test fails if a listed version stops passing.
//!
//! Run it with output:  cargo test --release -p vf-core --test engine_compat -- --nocapture
//! Engines that are not on the machine are skipped, so the test passes on a PC without ArtCraft.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use vf_core::engine::{self, DECKCRAFT, Engine, GRIDCRAFT, PDFCRAFT, WORDCRAFT};
use vf_core::job::{self, Spec};
use vf_core::pdf;

struct Candidate {
    engine: &'static Engine,
    source: &'static str,
    path: PathBuf,
    version: String,
}

fn pages(pdf_path: &Path) -> usize {
    lopdf::Document::load(pdf_path).map(|d| d.get_pages().len()).unwrap_or(0)
}

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn candidates(e: &'static Engine) -> Vec<Candidate> {
    let mut list = Vec::new();
    let pinned = src_dir().join("engines").join(e.folder.to_lowercase()).join(e.cli);
    for (source, path) in [("pinned", Some(pinned)), ("installed", e.find_system())] {
        let Some(path) = path.filter(|p| p.is_file()) else { continue };
        let Some(version) = engine::version_of(&path) else { continue };
        list.push(Candidate { engine: e, source, path, version });
    }
    list
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vf_compat_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn convert(cli: &Path, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    job::convert_one(&Spec { cli: cli.to_path_buf(), out_ext: "pdf".into(), extra_args: vec![], env: vec![] }, src, out_dir)
}

fn rtf_pdf(wordcraft: &Path, dir: &Path) -> Result<PathBuf, String> {
    let rtf = dir.join("Prova con spazi e\u{300}.rtf");
    fs::write(&rtf, br"{\rtf1\ansi\deff0 Prima pagina caff\'e8.\page Seconda pagina.\page Terza pagina.\par}").map_err(|e| e.to_string())?;
    convert(wordcraft, &rtf, dir)
}

fn smoke_word(cli: &Path, dir: &Path) -> Result<(), String> {
    let pdf = rtf_pdf(cli, dir)?;
    match pages(&pdf) {
        3 => Ok(()),
        n => Err(format!("expected 3 pages, got {n}")),
    }
}

fn smoke_grid(cli: &Path, dir: &Path) -> Result<(), String> {
    let csv = dir.join("Citt\u{e0} e totali.csv");
    fs::write(&csv, "Citt\u{e0},Totale\nCaff\u{e8},12\nT\u{e8},7\n").map_err(|e| e.to_string())?;
    let pdf = convert(cli, &csv, dir)?;
    (pages(&pdf) >= 1).then_some(()).ok_or_else(|| "the PDF has no pages".to_owned())
}

fn smoke_deck(cli: &Path, dir: &Path) -> Result<(), String> {
    // A two-slide .pptx made by the engine itself, then converted back to PDF: the path the Slides app takes.
    let pptx = dir.join("Presentazione di prova.pptx");
    let made = Command::new(cli)
        .current_dir(dir)
        .args(["run", "--cmd", r#"slide.new={"title":"Prova caffè"}"#, "--cmd", r#"slide.new={"title":"Seconda"}"#, "--save"])
        .arg(&pptx)
        .output()
        .map_err(|e| e.to_string())?;
    if !made.status.success() || !pptx.is_file() {
        return Err(format!("could not make a test .pptx: {}", String::from_utf8_lossy(&made.stderr).lines().last().unwrap_or("")));
    }
    let pdf = convert(cli, &pptx, dir)?;
    // A new presentation starts with its title slide, so two `slide.new` make three slides (checked with `deckcraft-cli info`).
    match pages(&pdf) {
        3 => Ok(()),
        n => Err(format!("expected 3 pages, got {n}")),
    }
}

fn smoke_pdf(cli: &Path, dir: &Path, wordcraft: &Path) -> Result<(), String> {
    use pdf::{Export, Pass, SplitBy};
    let source = rtf_pdf(wordcraft, dir)?;
    let out = dir.join("out");
    for what in [Export::Docx, Export::Pptx, Export::Html, Export::Rtf, Export::Txt] {
        let f = pdf::export(cli, &source, what, &out).map_err(|e| format!("{what:?}: {e}"))?;
        if fs::metadata(&f).map(|m| m.len()).unwrap_or(0) == 0 {
            return Err(format!("{what:?} wrote an empty file"));
        }
    }
    let png = pdf::export(cli, &source, Export::Png { dpi: 72 }, &out)?;
    if fs::read_dir(&png).map(|d| d.count()).unwrap_or(0) != 3 {
        return Err("PNG: not one picture per page".to_owned());
    }
    pdf::merge(cli, &[source.clone(), source.clone()], &out, "merged.pdf").map_err(|e| format!("merge: {e}"))?;
    let parts = pdf::split(cli, &source, &SplitBy::Every(1), &out).map_err(|e| format!("split: {e}"))?;
    if fs::read_dir(&parts).map(|d| d.count()).unwrap_or(0) != 3 {
        return Err("split: not three parts".to_owned());
    }
    pdf::extract(cli, &source, "1-2", &out).map_err(|e| format!("extract: {e}"))?;
    pdf::batch(cli, &source, Pass::Shrink { ppi: 100, quality: 50 }, &out).map_err(|e| format!("shrink: {e}"))?;
    pdf::batch(cli, &source, Pass::Ocr, &out).map_err(|e| format!("ocr: {e}"))?;
    if pdf::extract(cli, &source, "9", &out).is_ok() {
        return Err("a page that does not exist was accepted".to_owned());
    }
    Ok(())
}

/// engine name (lower case) -> versions listed as tested, from `src/engine-compat.json`.
fn listed() -> std::collections::BTreeMap<String, Vec<String>> {
    let text = fs::read_to_string(src_dir().join("engine-compat.json")).unwrap_or_else(|_| "{}".to_owned());
    let v: serde_json::Value = serde_json::from_str(&text).expect("engine-compat.json is valid JSON");
    v.as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| (k.clone(), v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn engine_matrix() {
    let listed = listed();
    // PDF operations need a PDF to work on: any WordCraft will do for that.
    let word_for_fixture = candidates(&WORDCRAFT).into_iter().next().map(|c| c.path);
    let mut regressions = Vec::new();
    println!("\n{:<10} {:<10} {:<8} {:<8} result", "engine", "source", "version", "listed");
    for e in [&WORDCRAFT, &GRIDCRAFT, &DECKCRAFT, &PDFCRAFT] {
        let name = e.folder.to_lowercase();
        let tested = listed.get(&name).cloned().unwrap_or_default();
        let found = candidates(e);
        if found.is_empty() {
            println!("{name:<10} (not on this machine)");
        }
        for c in found {
            let dir = scratch(&format!("{name}_{}", c.source));
            let result = match name.as_str() {
                "wordcraft" => smoke_word(&c.path, &dir),
                "gridcraft" => smoke_grid(&c.path, &dir),
                "deckcraft" => smoke_deck(&c.path, &dir),
                _ => match &word_for_fixture {
                    Some(w) => smoke_pdf(&c.path, &dir, w),
                    None => Err("skipped: no WordCraft to make the test PDF".to_owned()),
                },
            };
            let is_listed = tested.contains(&c.version);
            println!(
                "{:<10} {:<10} {:<8} {:<8} {}",
                c.engine.folder.to_lowercase(),
                c.source,
                c.version,
                if is_listed { "yes" } else { "no" },
                match &result {
                    Ok(()) => "PASS".to_owned(),
                    Err(e) => format!("FAIL: {e}"),
                }
            );
            if is_listed && result.as_ref().is_err_and(|e| !e.starts_with("skipped")) {
                regressions.push(format!("{name} {} ({}) is listed as compatible but fails", c.version, c.source));
            }
            let _ = fs::remove_dir_all(&dir);
        }
    }
    assert!(regressions.is_empty(), "{}", regressions.join("; "));
}
