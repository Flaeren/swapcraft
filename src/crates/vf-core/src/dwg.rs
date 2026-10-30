//! DWG to PDF on top of `cadcraft-cli`: the drawing's sheets (one PDF per layout, merged with bookmarks
//! or kept apart), or the whole Model space on one page that has the drawing's own proportions.
//!
//! Notes on the whole-drawing mode, all found by testing the engine (the old Python version's notes are gone with it):
//! line weights are off (at this scale one pen width swamps the geometry); the paper is the A size whose
//! result lands closest to the wanted long side; the page box is then cut to the drawing. Scaling the
//! page afterwards is NOT done: it makes the content stream huge.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::job::{deliver, run_capture, run_command, scratch_dir, unique_path};
use crate::pdfmerge::{crop_first_page, merge_with_bookmarks};
use crate::tf;

/// Wanted long side of the finished page in mm (the export is vector: this only sets the page size).
pub const SIZES: [(&str, f64); 4] = [("A3 (420 mm)", 420.0), ("A2 (594 mm)", 594.0), ("A1 (841 mm)", 841.0), ("A0 (1189 mm)", 1189.0)];
/// Paper name, short side, long side (mm).
const PAPERS: [(&str, f64, f64); 5] =
    [("A0", 841.0, 1189.0), ("A1", 594.0, 841.0), ("A2", 420.0, 594.0), ("A3", 297.0, 420.0), ("A4", 210.0, 297.0)];
/// Device margin on every side of the "CADCraft PDF" plotter.
const MARGIN: f64 = 5.0;
const PT_PER_MM: f64 = 72.0 / 25.4;

#[derive(Clone, Copy, Debug)]
pub struct SheetOptions {
    /// Also add the Model space when the drawing has layouts.
    pub include_model: bool,
    /// One PDF for the drawing (otherwise one per sheet, in a folder).
    pub merge: bool,
}

struct Sheet {
    layout: String,
    path: PathBuf,
}

fn cadcraft(cli: &Path, dwg: &Path, cmds: &[String]) -> Command {
    let mut cmd = Command::new(cli);
    cmd.arg("run").arg(dwg);
    for c in cmds {
        cmd.arg("--cmd").arg(c);
    }
    cmd
}

/// JSON values printed one per line by the engine.
fn json_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('{') || l.starts_with('['))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn forward(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn publish(cli: &Path, dwg: &Path, tmp: &Path) -> Result<Vec<Sheet>, String> {
    let target = forward(&tmp.join("sheet.pdf"));
    let mut cmd = cadcraft(cli, dwg, &[format!("publish {}", json!({ "path": target }))]);
    let out = run_capture(&mut cmd, tmp)?;
    let sheets = json_lines(&out)
        .into_iter()
        .find_map(|v| v.get("sheets").cloned())
        .and_then(|s| s.as_array().cloned())
        .ok_or("the engine reported no sheets")?;
    Ok(sheets
        .iter()
        .filter_map(|s| Some(Sheet { layout: s.get("layout")?.as_str()?.to_owned(), path: PathBuf::from(s.get("path")?.as_str()?) }))
        .collect())
}

/// The drawing's sheets as PDF. Returns the new file, or the folder holding one file per sheet.
pub fn convert_sheets(cli: &Path, dwg: &Path, out_dir: &Path, opts: SheetOptions, notify: &dyn Fn(String)) -> Result<PathBuf, String> {
    let tmp = scratch_dir().map_err(|e| e.to_string())?;
    let result = sheets_in(&tmp, cli, dwg, out_dir, opts, notify);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn sheets_in(tmp: &Path, cli: &Path, dwg: &Path, out_dir: &Path, opts: SheetOptions, notify: &dyn Fn(String)) -> Result<PathBuf, String> {
    let mut sheets = publish(cli, dwg, tmp)?;
    if sheets.iter().any(|s| s.layout != "Model") && !opts.include_model {
        sheets.retain(|s| s.layout != "Model");
    }
    if sheets.is_empty() {
        return Err(crate::t("dwg.no_sheets"));
    }
    notify(tf("dwg.sheets_note", &[("n", &sheets.len().to_string())]));
    let stem = dwg.file_stem().and_then(|s| s.to_str()).unwrap_or("drawing").to_owned();
    if opts.merge || sheets.len() == 1 {
        let out = tmp.join("merged.pdf");
        if sheets.len() == 1 {
            fs::copy(&sheets[0].path, &out).map_err(|e| e.to_string())?;
        } else {
            let parts: Vec<(String, &Path)> = sheets.iter().map(|s| (s.layout.clone(), s.path.as_path())).collect();
            merge_with_bookmarks(&parts, &out)?;
        }
        return deliver(&out, out_dir, &format!("{stem}.pdf"));
    }
    let folder = out_dir.join(&stem);
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    for s in &sheets {
        let safe: String = s.layout.chars().map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
        let to = unique_path(&folder.join(format!("{stem} - {safe}.pdf")));
        fs::copy(&s.path, to).map_err(|e| e.to_string())?;
    }
    Ok(folder)
}

type Extents = (f64, f64, f64, f64);

fn extents(cli: &Path, dwg: &Path, tmp: &Path) -> Result<Extents, String> {
    let mut cmd = Command::new(cli);
    cmd.arg("info").arg(dwg);
    let out = run_capture(&mut cmd, tmp)?;
    let v: Value = serde_json::from_str(out.trim()).map_err(|e| format!("info: {e}"))?;
    let n = |a: usize, b: usize| v["extents"][a][b].as_f64().ok_or_else(|| "the drawing reports no extents".to_owned());
    Ok((n(0, 0)?, n(0, 1)?, n(1, 0)?, n(1, 1)?))
}

/// Export geometry for `fit` on a paper in the drawing's orientation: page size (mm), scale (mm per
/// drawing unit) and the origin of the drawing on the page.
fn fit(ew: f64, eh: f64, paper: (&str, f64, f64)) -> (f64, f64, f64, f64, f64) {
    let (_, short, long) = paper;
    let (pw, ph) = if ew >= eh { (long, short) } else { (short, long) };
    let s = ((pw - 2.0 * MARGIN) / ew).min((ph - 2.0 * MARGIN) / eh);
    let ox = MARGIN + ((pw - 2.0 * MARGIN) - ew * s) / 2.0;
    let oy = MARGIN + ((ph - 2.0 * MARGIN) - eh * s) / 2.0;
    (pw, ph, s, ox, oy)
}

/// One representative point for an entity.
fn rep_point(e: &Value) -> Option<(f64, f64)> {
    let g = e.get("geometry")?;
    let xy = |v: &Value| Some((v.get("x")?.as_f64()?, v.get("y")?.as_f64()?));
    if e.get("type").and_then(Value::as_str) == Some("Line")
        && let (Some(a), Some(b)) = (g.get("a").and_then(xy), g.get("b").and_then(xy))
    {
        return Some(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
    }
    for key in ["insert", "center", "defpt", "origin"] {
        if let Some(p) = g.get(key).filter(|v| v.is_object()).and_then(xy) {
            return Some(p);
        }
    }
    if let Some(p) = g.get("vertices").and_then(|v| v.get(0)).and_then(|v| v.get("p")).and_then(xy) {
        return Some(p);
    }
    g.get("control").and_then(|v| v.get(0)).and_then(xy)
}

/// Where the drawing really is: the box of the grid cells (1/80 of the visible extents) that hold at
/// least 5 entities, plus 5% around, clamped to the extents. Only entities inside the extents count.
/// If more than 1% of the entities would fall outside the area is not clearly separated: no crop.
fn content_window(cli: &Path, dwg: &Path, tmp: &Path, ext: Extents) -> Result<Option<(Extents, usize)>, String> {
    let mut cmd = cadcraft(cli, dwg, &[format!("entities {}", json!({ "limit": 10_000_000 }))]);
    let out = run_capture(&mut cmd, tmp)?;
    let list = json_lines(&out).into_iter().find_map(|v| v.get("entities").and_then(|e| e.as_array().cloned()));
    let pts: Vec<(f64, f64)> = list
        .unwrap_or_default()
        .iter()
        .filter_map(rep_point)
        .filter(|p| ext.0 <= p.0 && p.0 <= ext.2 && ext.1 <= p.1 && p.1 <= ext.3)
        .collect();
    if pts.len() < 100 {
        return Ok(None);
    }
    let cell = {
        let c = (ext.2 - ext.0).max(ext.3 - ext.1) / 80.0;
        if c == 0.0 { 1.0 } else { c }
    };
    let mut cells: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for (x, y) in &pts {
        *cells.entry((((x - ext.0) / cell) as i64, ((y - ext.1) / cell) as i64)).or_default() += 1;
    }
    let dense: Vec<(i64, i64)> = cells.into_iter().filter(|(_, n)| *n >= 5).map(|(c, _)| c).collect();
    if dense.is_empty() {
        return Ok(None);
    }
    let (cx0, cx1) = (dense.iter().map(|c| c.0).min().unwrap(), dense.iter().map(|c| c.0).max().unwrap());
    let (cy0, cy1) = (dense.iter().map(|c| c.1).min().unwrap(), dense.iter().map(|c| c.1).max().unwrap());
    let (bx0, by0) = (ext.0 + cx0 as f64 * cell, ext.1 + cy0 as f64 * cell);
    let (bx1, by1) = (ext.0 + (cx1 + 1) as f64 * cell, ext.1 + (cy1 + 1) as f64 * cell);
    let pad = 0.05 * (bx1 - bx0).max(by1 - by0);
    let win = ((bx0 - pad).max(ext.0), (by0 - pad).max(ext.1), (bx1 + pad).min(ext.2), (by1 + pad).min(ext.3));
    if win.2 - win.0 <= 0.0 || win.3 - win.1 <= 0.0 {
        return Ok(None);
    }
    let left_out = pts.iter().filter(|(x, y)| !(win.0 <= *x && *x <= win.2 && win.1 <= *y && *y <= win.3)).count();
    if left_out as f64 > 0.01 * pts.len() as f64 {
        return Ok(None);
    }
    Ok(Some((win, left_out)))
}

/// The whole drawing (Model space) on ONE page with the drawing's proportions. `paper_label` is only
/// used in the log line.
pub fn convert_whole(
    cli: &Path, dwg: &Path, out_dir: &Path, long_side_mm: f64, crop: bool, paper_label: &str, notify: &dyn Fn(String),
) -> Result<PathBuf, String> {
    let tmp = scratch_dir().map_err(|e| e.to_string())?;
    let result = whole_in(&tmp, cli, dwg, out_dir, long_side_mm, crop, paper_label, notify);
    let _ = fs::remove_dir_all(&tmp);
    result
}

#[allow(clippy::too_many_arguments)]
fn whole_in(
    tmp: &Path, cli: &Path, dwg: &Path, out_dir: &Path, long_side_mm: f64, crop: bool, paper_label: &str, notify: &dyn Fn(String),
) -> Result<PathBuf, String> {
    let ext = extents(cli, dwg, tmp)?;
    let (ew, eh) = (ext.2 - ext.0, ext.3 - ext.1);
    if ew <= 0.0 || eh <= 0.0 {
        return Err(crate::t("dwg.no_sheets"));
    }
    let window = if crop { content_window(cli, dwg, tmp, ext)? } else { None };
    let (win, left_out) = window.unwrap_or((ext, 0));
    let crop_long = (win.2 - win.0).max(win.3 - win.1);

    let paper = *PAPERS
        .iter()
        .min_by(|a, b| {
            let da = (crop_long * fit(ew, eh, **a).2 / long_side_mm - 1.0).abs();
            let db = (crop_long * fit(ew, eh, **b).2 / long_side_mm - 1.0).abs();
            da.total_cmp(&db)
        })
        .expect("papers are not empty");
    let (pw, ph, s, ox, oy) = fit(ew, eh, paper);
    let model = tmp.join("model.pdf");
    let export = json!({
        "path": forward(&model), "layout": "Model", "paper": paper.0,
        "landscape": ew >= eh, "fit": true, "lineweights": false,
    });
    let mut cmd = cadcraft(cli, dwg, &[format!("exportpdf {export}")]);
    run_command(&mut cmd, tmp)?;
    if !model.is_file() {
        return Err(crate::t("dwg.export_failed"));
    }

    let pad = 0.01 * crop_long * s; // mm of breathing room around the drawing
    let left = (ox + (win.0 - ext.0) * s - pad).max(0.0);
    let right = (ox + (win.2 - ext.0) * s + pad).min(pw);
    let top = (oy + (ext.3 - win.3) * s - pad).max(0.0); // from the page top
    let bottom = (oy + (ext.3 - win.1) * s + pad).min(ph);
    let rect = [left * PT_PER_MM, (ph - bottom) * PT_PER_MM, right * PT_PER_MM, (ph - top) * PT_PER_MM];
    let cropped = tmp.join("cropped.pdf");
    crop_first_page(&model, &cropped, rect)?;
    notify(tf("dwg.whole_note", &[("paper", paper_label)]));
    if window.is_some() && left_out > 0 {
        notify(tf("dwg.crop_note", &[("n", &left_out.to_string())]));
    }
    let stem = dwg.file_stem().and_then(|s| s.to_str()).unwrap_or("drawing");
    deliver(&cropped, out_dir, &format!("{stem}.pdf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_drawing_inside_the_margins() {
        let (pw, ph, s, ox, oy) = fit(1000.0, 500.0, PAPERS[3]);
        assert_eq!((pw, ph), (420.0, 297.0));
        assert!(ox >= MARGIN - 1e-9 && oy >= MARGIN - 1e-9);
        assert!(1000.0 * s + 2.0 * ox <= pw + 1e-9);
        assert!(500.0 * s + 2.0 * oy <= ph + 1e-9);
    }

    /// Real CADCraft on its own sample drawings (skipped when no engine is found): every mode yields a
    /// readable PDF; the merged one has one bookmark per sheet; the whole-drawing page keeps the drawing's
    /// proportions.
    #[test]
    fn real_engine_every_mode() {
        let Some(cli) = crate::engine::CADCRAFT.find() else { return };
        let dir = std::env::temp_dir().join("vf_dwg_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let none = |_: String| {};
        for sample in ["floorplan", "bracket"] {
            let dwg = dir.join(format!("{sample}.dwg"));
            let status = Command::new(&cli).args(["sample", sample]).arg(&dwg).output().expect("run the engine");
            assert!(status.status.success() && dwg.is_file(), "sample {sample}");
            let out = dir.join("out");
            let merged = convert_sheets(&cli, &dwg, &out, SheetOptions { include_model: true, merge: true }, &none).expect("merged sheets");
            let doc = lopdf::Document::load(&merged).expect("a readable merged pdf");
            let pages = doc.get_pages().len();
            assert!(pages >= 1);
            // the Model page (first) must keep its drawing content through the merge
            let first = *doc.get_pages().values().next().unwrap();
            assert!(doc.get_page_content(first).len() > 100, "{sample}: the merged Model page has content");
            if pages >= 3 {
                assert_eq!(doc.get_pages().len(), pages);
            }
            if pages > 1 {
                let catalog = doc.catalog().unwrap();
                assert!(catalog.get(b"Outlines").is_ok(), "bookmarks on a multi-sheet file");
            }
            let split = convert_sheets(&cli, &dwg, &out, SheetOptions { include_model: true, merge: false }, &none).expect("split sheets");
            assert!(fs::read_dir(&split).unwrap().count() >= 1);
            for crop in [false, true] {
                let whole = convert_whole(&cli, &dwg, &out, 420.0, crop, "A3", &none).expect("whole drawing");
                let d = lopdf::Document::load(&whole).expect("a readable page");
                assert_eq!(d.get_pages().len(), 1);
                let page = d.get_page_content(*d.get_pages().values().next().unwrap());
                assert!(!page.is_empty(), "the page has drawing content");
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Manual run on a real drawing: `VF_DWG=<file> VF_OUT=<folder> cargo test --release -p vf-core real_drawing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_drawing() {
        let (Ok(dwg), Ok(out)) = (std::env::var("VF_DWG"), std::env::var("VF_OUT")) else { return };
        let (dwg, out) = (PathBuf::from(dwg), PathBuf::from(out));
        let cli = crate::engine::CADCRAFT.find().expect("an engine");
        let log = |m: String| println!("  note: {m}");
        let t = std::time::Instant::now();
        let merged = convert_sheets(&cli, &dwg, &out, SheetOptions { include_model: true, merge: true }, &log).expect("merged");
        println!("merged {:?} in {:.1}s", merged, t.elapsed().as_secs_f64());
        let t = std::time::Instant::now();
        let whole = convert_whole(&cli, &dwg, &out, 1189.0, true, "A0", &log).expect("whole");
        println!("whole {:?} in {:.1}s", whole, t.elapsed().as_secs_f64());
    }

    #[test]
    fn representative_points() {
        let line = json!({"type": "Line", "geometry": {"a": {"x": 0.0, "y": 0.0}, "b": {"x": 10.0, "y": 4.0}}});
        assert_eq!(rep_point(&line), Some((5.0, 2.0)));
        let circle = json!({"type": "Circle", "geometry": {"center": {"x": 3.0, "y": 7.0}}});
        assert_eq!(rep_point(&circle), Some((3.0, 7.0)));
        assert_eq!(rep_point(&json!({"type": "X", "geometry": {}})), None);
    }
}
