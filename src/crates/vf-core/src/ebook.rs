//! E-book conversions as the apps, the command line and the MCP server see them: a file in, a new file out.
//!
//! EPUB and Kindle files (AZW3, AZW, MOBI, PRC) are read in Rust (`epub.rs`, `mobi.rs`); so are text and HTML.
//! Word processor files and Markdown go through WordCraft to HTML first. EPUB, text and HTML are written in
//! Rust; PDF and DOCX are made by WordCraft from the book's HTML. Books with DRM are refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::epub::{self, Book};
use crate::job::{self, Spec};
use crate::mobi;

/// Files read straight into a book, no engine needed.
pub const KINDLE_EXTS: &[&str] = &["azw3", "azw", "mobi", "prc", "kf8"];
/// Files that come to a book through WordCraft (word processor files and Markdown).
pub const WORDCRAFT_EXTS: &[&str] = &["docx", "doc", "odt", "rtf", "md"];
/// Everything the e-book converter takes.
pub const SOURCE_EXTS: &[&str] = &["epub", "azw3", "azw", "mobi", "prc", "kf8", "txt", "html", "htm", "xhtml", "md", "docx", "doc", "odt", "rtf"];

/// What a conversion makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Epub,
    Txt,
    Html,
    Pdf,
    Docx,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Epub => "epub",
            Kind::Txt => "txt",
            Kind::Html => "html",
            Kind::Pdf => "pdf",
            Kind::Docx => "docx",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Kind> {
        [Kind::Epub, Kind::Txt, Kind::Html, Kind::Pdf, Kind::Docx].into_iter().find(|k| k.ext() == ext)
    }

    /// Whether WordCraft is needed to make it.
    pub fn needs_wordcraft(self) -> bool {
        matches!(self, Kind::Pdf | Kind::Docx)
    }
}

fn lower_ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

/// Text file bytes as text: UTF-8 (with or without a byte-order mark), else Windows-1252.
fn text_from_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{feff}').to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

fn wordcraft_to_html(cli: &Path, src: &Path, tmp: &Path) -> Result<PathBuf, String> {
    if cli.as_os_str().is_empty() {
        return Err("The WordCraft engine is needed to read this kind of file, and it was not found.".to_owned());
    }
    // a plain name inside the scratch folder, so accents and odd spaces never reach the engine
    let input = tmp.join(format!("in.{}", lower_ext(src)));
    fs::copy(src, &input).map_err(|e| e.to_string())?;
    let out = tmp.join("in.html");
    let mut cmd = Command::new(cli);
    cmd.arg("convert").arg(&input).arg(&out);
    job::run_command(&mut cmd, tmp)?;
    if out.is_file() { Ok(out) } else { Err("the engine finished but wrote no file".to_owned()) }
}

/// Reads any supported file into a book. `tmp` is a scratch folder for the engine; `wordcraft` may be empty
/// when only Rust-read formats are used.
pub fn load(src: &Path, wordcraft: &Path, tmp: &Path) -> Result<Book, String> {
    let ext = lower_ext(src);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("Book").to_owned();
    let bytes = |p: &Path| fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    match ext.as_str() {
        "epub" => epub::read_epub(&bytes(src)?),
        e if KINDLE_EXTS.contains(&e) => mobi::read_mobi(&bytes(src)?),
        "txt" => Ok(epub::from_text(&text_from_bytes(&bytes(src)?), &stem)),
        "html" | "htm" | "xhtml" => Ok(epub::from_html(&text_from_bytes(&bytes(src)?), src.parent(), &stem)),
        e if WORDCRAFT_EXTS.contains(&e) => {
            let html = wordcraft_to_html(wordcraft, src, tmp)?;
            let mut book = epub::from_html(&text_from_bytes(&bytes(&html)?), Some(tmp), &stem);
            // a document's title is its file name unless the document said something better
            if book.title.is_empty() || book.title == "in" {
                book.title = stem;
            }
            Ok(book)
        }
        other => Err(format!("an e-book converter does not read .{other} files")),
    }
}

/// Converts one file. `spec.cli` is WordCraft (may be empty when the job needs none), `spec.out_ext` the target.
pub fn convert(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let kind = Kind::from_ext(&spec.out_ext).ok_or_else(|| format!("the e-book converter does not make .{} files", spec.out_ext))?;
    if lower_ext(src) == kind.ext() {
        return Err(format!("this is already a .{} file", kind.ext()));
    }
    let tmp = job::scratch_dir().map_err(|e| e.to_string())?;
    let result = run_in(&tmp, kind, spec, src, out_dir);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn run_in(tmp: &Path, kind: Kind, spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let book = load(src, &spec.cli, tmp)?;
    let target = tmp.join(format!("out.{}", kind.ext()));
    let io = |e: std::io::Error| e.to_string();
    match kind {
        Kind::Epub => fs::write(&target, book.to_epub()?).map_err(io)?,
        Kind::Txt => fs::write(&target, book.to_text()).map_err(io)?,
        Kind::Html => fs::write(&target, book.to_html()).map_err(io)?,
        Kind::Pdf | Kind::Docx => {
            if spec.cli.as_os_str().is_empty() {
                return Err("The WordCraft engine is needed to make this, and it was not found.".to_owned());
            }
            let page = tmp.join("book.html");
            fs::write(&page, book.to_html()).map_err(io)?;
            let mut cmd = Command::new(&spec.cli);
            cmd.arg("convert").arg(&page).arg(&target);
            job::run_command(&mut cmd, tmp)?;
        }
    }
    if !target.is_file() {
        return Err("the conversion finished but wrote no file".to_owned());
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("book");
    job::deliver(&target, out_dir, &format!("{stem}.{}", kind.ext()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_extensions() {
        assert_eq!(Kind::from_ext("epub"), Some(Kind::Epub));
        assert_eq!(Kind::from_ext("azw3"), None, "AZW3 is read, never written");
        assert!(Kind::Pdf.needs_wordcraft() && !Kind::Epub.needs_wordcraft());
    }

    #[test]
    fn text_files_in_old_encodings_are_read() {
        assert_eq!(text_from_bytes("caff\u{e8}".as_bytes()), "caff\u{e8}");
        assert_eq!(text_from_bytes(b"caff\xe8"), "caff\u{e8}", "Windows-1252 / Latin-1");
        assert_eq!(text_from_bytes(b"\xef\xbb\xbfhi"), "hi");
    }

    /// With the real WordCraft (skipped without it): a book becomes PDF and Word, a Word file and a Markdown
    /// file become EPUBs, and the words survive the round trip.
    #[test]
    fn real_engine_pdf_docx_and_back() {
        let Some(cli) = crate::engine::WORDCRAFT.find() else { return };
        let dir = std::env::temp_dir().join(format!("vf_ebook_engine_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let spec = |ext: &str| Spec { cli: cli.clone(), out_ext: ext.into(), extra_args: vec![], env: vec![] };
        let src = dir.join("Libro di prova.txt");
        fs::write(&src, "Il caff\u{e8} della sera.\n\nSeconda pagina con t\u{e8} e citt\u{e0}.\n").unwrap();
        let epub_path = convert(&spec("epub"), &src, &dir).unwrap();
        let pdf = convert(&spec("pdf"), &epub_path, &dir).expect("epub -> pdf");
        let docx = convert(&spec("docx"), &epub_path, &dir).expect("epub -> docx");
        assert!(fs::metadata(&pdf).unwrap().len() > 500 && fs::metadata(&docx).unwrap().len() > 500);
        let words = |p: &Path| -> Vec<String> {
            let t = fs::read_to_string(p).unwrap().to_lowercase();
            t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect()
        };
        // Word -> EPUB -> text keeps the words
        let back_epub = convert(&spec("epub"), &docx, &dir.join("b")).expect("docx -> epub");
        let back_txt = convert(&spec("txt"), &back_epub, &dir.join("b")).expect("epub -> txt");
        let (w, original) = (words(&back_txt), words(&convert(&spec("txt"), &epub_path, &dir.join("o")).unwrap()));
        for needed in ["caff\u{e8}", "sera", "citt\u{e0}"] {
            assert!(w.iter().any(|x| x == needed), "{needed} survived docx -> epub: {w:?}");
        }
        assert!(original.iter().any(|x| x == "seconda"));
        // Markdown -> EPUB
        let md = dir.join("Note.md");
        fs::write(&md, "# Titolo\n\nUn paragrafo con **grassetto**.\n").unwrap();
        let md_epub = convert(&spec("epub"), &md, &dir.join("m")).expect("md -> epub");
        let md_txt = convert(&spec("txt"), &md_epub, &dir.join("m")).unwrap();
        let text = fs::read_to_string(md_txt).unwrap();
        assert!(text.contains("Un paragrafo con grassetto."), "{text:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_to_epub_to_text_without_any_engine() {
        let dir = std::env::temp_dir().join(format!("vf_ebook_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Il mio libro \u{e0}.txt");
        fs::write(&src, "Una riga\ncontinua.\n\nSeconda: caff\u{e8} & t\u{e8}.\n").unwrap();
        let spec = |ext: &str| Spec { cli: PathBuf::new(), out_ext: ext.into(), extra_args: vec![], env: vec![] };
        let epub_path = convert(&spec("epub"), &src, &dir).expect("txt -> epub");
        let html = convert(&spec("html"), &epub_path, &dir).expect("epub -> html");
        assert!(fs::read_to_string(&html).unwrap().contains("caff\u{e8} &amp; t\u{e8}"));
        let txt = convert(&spec("txt"), &epub_path, &dir.join("o")).expect("epub -> txt");
        let text = fs::read_to_string(&txt).unwrap();
        assert!(text.contains("Una riga continua.\n\nSeconda: caff\u{e8} & t\u{e8}."), "{text:?}");
        assert!(convert(&spec("epub"), &epub_path, &dir).unwrap_err().contains("already"));
        assert!(convert(&spec("pdf"), &epub_path, &dir).unwrap_err().contains("WordCraft"), "PDF needs the engine");
        assert!(convert(&spec("azw3"), &src, &dir).is_err(), "AZW3 is never written");
        let _ = fs::remove_dir_all(&dir);
    }
}
