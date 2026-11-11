//! PDF operations on top of `pdfcraft-cli`. Every call works in a scratch folder on copies with plain
//! names (`in.pdf`, `out.docx`), so accents and spaces in the user's names never reach the engine, and
//! the result is moved into place only when the engine succeeded.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::job::{deliver, run_command, scratch_dir, unique_path};

/// What "convert" can write. Word, PowerPoint, HTML and RTF come from PdfCraft's office export (text,
/// headings, bold/italic and inferred tables; vector graphics and exact fonts are limited).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Export {
    Docx,
    Pptx,
    Html,
    Rtf,
    Txt,
    Png { dpi: u32 },
    Jpeg { dpi: u32 },
}

impl Export {
    pub fn ext(self) -> &'static str {
        match self {
            Export::Docx => "docx",
            Export::Pptx => "pptx",
            Export::Html => "html",
            Export::Rtf => "rtf",
            Export::Txt => "txt",
            Export::Png { .. } => "png",
            Export::Jpeg { .. } => "jpg",
        }
    }
}

/// JSON string literal for `s` (paths on Windows are full of backslashes).
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn with_scratch<T>(f: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
    let tmp = scratch_dir().map_err(|e| e.to_string())?;
    let result = f(&tmp);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn stage(src: &Path, tmp: &Path, name: &str) -> Result<PathBuf, String> {
    let to = tmp.join(name);
    fs::copy(src, &to).map_err(|e| format!("{}: {e}", src.display()))?;
    Ok(to)
}

fn stem(src: &Path) -> String {
    src.file_stem().and_then(|s| s.to_str()).unwrap_or("out").to_owned()
}

/// Runs a two-step script: open `in.pdf`, then the given tool with `extra` JSON fields.
fn run_script(cli: &Path, tmp: &Path, tool: &str, extra: &str) -> Result<(), String> {
    let input = tmp.join("in.pdf");
    let script = format!(
        "[{{\"tool\":\"doc_open\",\"args\":{{\"path\":{}}}}},{{\"tool\":\"{tool}\",\"args\":{{\"doc\":1,{extra}}}}}]",
        json_str(&input.to_string_lossy())
    );
    let script_path = tmp.join("steps.json");
    fs::write(&script_path, script).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(cli);
    cmd.arg("run").arg("--script").arg(&script_path).arg("--root").arg(tmp);
    run_command(&mut cmd, tmp)
}

/// PDF to another format. Pictures go to a folder `<name> (pages)` next to (or in) `out_dir`; the
/// returned path is that folder, otherwise the new file.
pub fn export(cli: &Path, src: &Path, what: Export, out_dir: &Path) -> Result<PathBuf, String> {
    with_scratch(|tmp| {
        stage(src, tmp, "in.pdf")?;
        let ext = what.ext();
        match what {
            Export::Png { dpi } | Export::Jpeg { dpi } => {
                let format = if matches!(what, Export::Png { .. }) { "png" } else { "jpeg" };
                let folder = tmp.join("pages");
                run_script(cli, tmp, "doc_export_images", &format!("\"folder\":{},\"format\":\"{format}\",\"dpi\":{dpi}", json_str(&folder.to_string_lossy())))?;
                let name = stem(src);
                let target = unique_path(&out_dir.join(format!("{name} (pages)")));
                fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                let mut n = 0;
                for entry in fs::read_dir(&folder).map_err(|e| e.to_string())?.flatten() {
                    let file = entry.file_name().to_string_lossy().into_owned();
                    if let Some(rest) = file.strip_prefix("in_page_") {
                        fs::copy(entry.path(), target.join(format!("{name}_page_{rest}"))).map_err(|e| e.to_string())?;
                        n += 1;
                    }
                }
                if n == 0 {
                    let _ = fs::remove_dir(&target);
                    return Err("the engine wrote no pages".to_owned());
                }
                Ok(target)
            }
            _ => {
                let out = tmp.join(format!("out.{ext}"));
                let tool = if what == Export::Txt { "doc_export_text" } else { "doc_export_office" };
                run_script(cli, tmp, tool, &format!("\"path\":{}", json_str(&out.to_string_lossy())))?;
                if !out.is_file() {
                    return Err("the engine finished but wrote no file".to_owned());
                }
                deliver(&out, out_dir, &format!("{}.{ext}", stem(src)))
            }
        }
    })
}

/// Joins PDFs, in the order given, into `out_dir/name`.
pub fn merge(cli: &Path, files: &[PathBuf], out_dir: &Path, name: &str) -> Result<PathBuf, String> {
    with_scratch(|tmp| {
        let mut cmd = Command::new(cli);
        cmd.arg("combine");
        for (i, f) in files.iter().enumerate() {
            cmd.arg(stage(f, tmp, &format!("{i}.pdf"))?);
        }
        let out = tmp.join("out.pdf");
        cmd.arg("--out").arg(&out);
        run_command(&mut cmd, tmp)?;
        deliver(&out, out_dir, name)
    })
}

/// How to cut a PDF into parts.
#[derive(Clone, Debug)]
pub enum SplitBy {
    /// A new file every N pages.
    Every(u32),
    /// A new file starts at each of these pages, e.g. `3,7`.
    Before(String),
}

/// Splits into a folder `<name> (parts)`; returns that folder.
pub fn split(cli: &Path, src: &Path, by: &SplitBy, out_dir: &Path) -> Result<PathBuf, String> {
    with_scratch(|tmp| {
        let input = stage(src, tmp, "in.pdf")?;
        let parts = tmp.join("parts");
        let mut cmd = Command::new(cli);
        cmd.arg("split").arg(&input);
        match by {
            SplitBy::Every(n) => cmd.arg("--every").arg(n.to_string()),
            SplitBy::Before(list) => cmd.arg("--before").arg(list),
        };
        cmd.arg("--out-dir").arg(&parts);
        run_command(&mut cmd, tmp)?;
        let name = stem(src);
        let target = unique_path(&out_dir.join(format!("{name} (parts)")));
        fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let mut files: Vec<_> = fs::read_dir(&parts).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        files.sort();
        if files.is_empty() {
            let _ = fs::remove_dir(&target);
            return Err("the engine wrote no files".to_owned());
        }
        for (i, f) in files.iter().enumerate() {
            fs::copy(f, target.join(format!("{name} - part {}.pdf", i + 1))).map_err(|e| e.to_string())?;
        }
        Ok(target)
    })
}

/// Keeps only `pages` (e.g. `1,3,5-8`) in a new PDF.
pub fn extract(cli: &Path, src: &Path, pages: &str, out_dir: &Path) -> Result<PathBuf, String> {
    with_scratch(|tmp| {
        let input = stage(src, tmp, "in.pdf")?;
        let out = tmp.join("out.pdf");
        let mut cmd = Command::new(cli);
        cmd.arg("extract").arg(&input).arg("--pages").arg(pages).arg("--out").arg(&out);
        run_command(&mut cmd, tmp)?;
        deliver(&out, out_dir, &format!("{} (extract).pdf", stem(src)))
    })
}

/// A `batch` step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pass {
    /// Make scanned pages searchable. PdfCraft reads English only.
    Ocr,
    /// Smaller file: images at `ppi` and JPEG `quality`.
    Shrink { ppi: u32, quality: u32 },
}

pub fn batch(cli: &Path, src: &Path, pass: Pass, out_dir: &Path) -> Result<PathBuf, String> {
    with_scratch(|tmp| {
        let input = stage(src, tmp, "in.pdf")?;
        let done = tmp.join("done");
        let mut cmd = Command::new(cli);
        cmd.arg("batch").arg(&input).arg("--out-dir").arg(&done);
        let suffix = match pass {
            Pass::Ocr => {
                cmd.args(["--steps", "ocr", "--language", "en"]);
                "OCR"
            }
            Pass::Shrink { ppi, quality } => {
                cmd.args(["--steps", "optimize", "--ppi", &ppi.to_string(), "--quality", &quality.to_string()]);
                "smaller"
            }
        };
        run_command(&mut cmd, tmp)?;
        let produced = fs::read_dir(&done)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
            .ok_or_else(|| "the engine wrote no file".to_owned())?;
        deliver(&produced, out_dir, &format!("{} ({suffix}).pdf", stem(src)))
    })
}

/// The picture types PdfCraft puts on a page by itself.
const PAGE_IMAGES: &[&str] = &["png", "jpg", "tif", "bmp", "gif"];

/// A picture as a PDF with one page, the page the size of the picture. PdfCraft takes PNG, JPG, TIFF, BMP and GIF
/// as they are; any other picture type PhotoCraft reads (WebP, ICO, TGA, PSD...) goes through a PNG first.
pub fn from_image(pdfcraft: &Path, photocraft: Option<&Path>, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    with_scratch(|tmp| {
        let ext = match src.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default().as_str() {
            "jpeg" => "jpg".to_owned(),
            "tiff" => "tif".to_owned(),
            other => other.to_owned(),
        };
        let input = if PAGE_IMAGES.contains(&ext.as_str()) {
            stage(src, tmp, &format!("in.{ext}"))?
        } else {
            let photocraft = photocraft.ok_or("The PhotoCraft engine is needed to read this picture, and it was not found.")?;
            let from = stage(src, tmp, &format!("source.{ext}"))?;
            let png = tmp.join("in.png");
            let mut cmd = Command::new(photocraft);
            cmd.arg("convert").arg(&from).arg(&png);
            run_command(&mut cmd, tmp)?;
            png
        };
        let done = tmp.join("done");
        let mut cmd = Command::new(pdfcraft);
        cmd.arg("batch").arg(&input).args(["--steps", "convert"]).arg("--out-dir").arg(&done);
        run_command(&mut cmd, tmp)?;
        let produced = fs::read_dir(&done)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
            .ok_or_else(|| "the engine wrote no file".to_owned())?;
        deliver(&produced, out_dir, &format!("{}.pdf", stem(src)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs every operation on a real 3-page PDF made by WordCraft (skipped when an engine is missing).
    #[test]
    fn real_engines_every_operation() {
        use crate::engine::{PDFCRAFT, WORDCRAFT};
        use crate::job::{Spec, convert_one};
        let (Some(pdfcraft), Some(wordcraft)) = (PDFCRAFT.find(), WORDCRAFT.find()) else { return };
        let dir = std::env::temp_dir().join("vf_pdf_ops_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let rtf = dir.join("Prova con spazi e\u{300}.rtf");
        fs::write(&rtf, br"{\rtf1\ansi\deff0 Prima pagina caff\'e8.\page Seconda pagina.\page Terza pagina.\par}").unwrap();
        let spec = Spec { cli: wordcraft, out_ext: "pdf".into(), extra_args: vec![], env: vec![] };
        let pdf = convert_one(&spec, &rtf, &dir).expect("make a pdf");

        let out = dir.join("out");
        for what in [Export::Docx, Export::Pptx, Export::Html, Export::Rtf, Export::Txt] {
            let f = export(&pdfcraft, &pdf, what, &out).unwrap_or_else(|e| panic!("{what:?}: {e}"));
            assert!(f.is_file() && fs::metadata(&f).unwrap().len() > 0, "{what:?}");
        }
        let png = export(&pdfcraft, &pdf, Export::Png { dpi: 72 }, &out).expect("png");
        assert_eq!(fs::read_dir(&png).unwrap().count(), 3, "one picture per page");
        let jpg = export(&pdfcraft, &pdf, Export::Jpeg { dpi: 72 }, &out).expect("jpeg");
        assert_eq!(fs::read_dir(&jpg).unwrap().count(), 3);

        let merged = merge(&pdfcraft, &[pdf.clone(), pdf.clone()], &out, "merged.pdf").expect("merge");
        assert!(merged.is_file());
        let parts = split(&pdfcraft, &pdf, &SplitBy::Every(1), &out).expect("split");
        assert_eq!(fs::read_dir(&parts).unwrap().count(), 3);
        let first_two = extract(&pdfcraft, &pdf, "1-2", &out).expect("extract");
        assert!(first_two.is_file());
        assert!(batch(&pdfcraft, &pdf, Pass::Shrink { ppi: 100, quality: 50 }, &out).expect("shrink").is_file());
        assert!(batch(&pdfcraft, &pdf, Pass::Ocr, &out).expect("ocr").is_file());
        assert!(extract(&pdfcraft, &pdf, "9", &out).is_err(), "a page that does not exist is an error");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_escapes_windows_paths() {
        assert_eq!(json_str(r#"C:\a "b""#), r#""C:\\a \"b\"""#);
    }
}
