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

use vf_core::engine::{self, DECKCRAFT, Engine, FILMCRAFT, GRIDCRAFT, PDFCRAFT, PHOTOCRAFT, SOUNDCRAFT, WORDCRAFT};
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

/// A 16x8 24-bit BMP written by hand (no image library in the test).
fn tiny_bmp() -> Vec<u8> {
    let (w, h) = (16u32, 8u32);
    let size = 54 + w * 3 * h;
    let mut b = Vec::new();
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&h.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    for y in 0..h {
        for x in 0..w {
            b.extend_from_slice(&[(x * 16) as u8, (y * 30) as u8, 200]);
        }
    }
    b
}

fn smoke_photo(cli: &Path, dir: &Path) -> Result<(), String> {
    let src = dir.join("Foto di citt\u{e0}.bmp");
    fs::write(&src, tiny_bmp()).map_err(|e| e.to_string())?;
    for ext in ["png", "jpg", "webp", "tif", "gif", "ico"] {
        let spec = Spec { cli: cli.to_path_buf(), out_ext: ext.into(), extra_args: vec![], env: vec![] };
        let out = job::convert_one(&spec, &src, dir).map_err(|e| format!("bmp -> {ext}: {e}"))?;
        if fs::metadata(&out).map(|m| m.len()).unwrap_or(0) == 0 {
            return Err(format!("bmp -> {ext} wrote an empty file"));
        }
    }
    Ok(())
}

fn smoke_sound(cli: &Path, dir: &Path) -> Result<(), String> {
    // A quarter second of square wave as a 16-bit mono WAV, written by hand.
    let rate = 8000u32;
    let data_len = rate / 4 * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&[1, 0, 1, 0]);
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&[2, 0, 16, 0]);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend((0..data_len).map(|i| if i % 64 < 32 { 0x40 } else { 0xc0 }));
    let src = dir.join("Suono di citt\u{e0}.wav");
    fs::write(&src, wav).map_err(|e| e.to_string())?;
    for ext in ["flac", "aiff"] {
        let spec = Spec { cli: cli.to_path_buf(), out_ext: ext.into(), extra_args: vec!["--bit-depth".into(), "16".into()], env: vec![] };
        let out = job::convert_one(&spec, &src, dir).map_err(|e| format!("wav -> {ext}: {e}"))?;
        if fs::metadata(&out).map(|m| m.len()).unwrap_or(0) < 100 {
            return Err(format!("wav -> {ext} wrote a tiny file"));
        }
    }
    Ok(())
}

fn smoke_film(cli: &Path, dir: &Path) -> Result<(), String> {
    // A tiny clip made by the engine's own demo project, then the path the Video app takes: import, sequence, export.
    let src = dir.join("Gita di citt\u{e0}.mp4");
    let made = Command::new(cli).args(["--demo", "export"]).arg(&src).args(["--format", "h264", "--scale", "0.1"]).output().map_err(|e| e.to_string())?;
    if !made.status.success() || !src.is_file() {
        return Err("could not make the demo clip".to_owned());
    }
    for (ext, args) in [("mp4", vec!["format=h264", "bitrateMode=crf", "crf=28"]), ("mov", vec!["format=prores"]), ("wav", vec!["format=wav"])] {
        let spec = Spec { cli: cli.to_path_buf(), out_ext: ext.into(), extra_args: args.into_iter().map(String::from).collect(), env: vec![] };
        let out = vf_core::film::convert(&spec, &src, dir).map_err(|e| format!("mp4 -> {ext}: {e}"))?;
        if fs::metadata(&out).map(|m| m.len()).unwrap_or(0) < 1000 {
            return Err(format!("mp4 -> {ext} wrote a tiny file"));
        }
    }
    Ok(())
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
    for e in [&WORDCRAFT, &GRIDCRAFT, &DECKCRAFT, &PDFCRAFT, &PHOTOCRAFT, &SOUNDCRAFT, &FILMCRAFT] {
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
                "photocraft" => smoke_photo(&c.path, &dir),
                "soundcraft" => smoke_sound(&c.path, &dir),
                "filmcraft" => smoke_film(&c.path, &dir),
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
