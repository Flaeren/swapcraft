//! What voidflare can convert: every route says which extensions it takes, what it makes and which engine
//! does it. The drop panel offers the routes matching the dropped file. Merge, split, OCR and the other
//! options stay in the apps; routes run with the apps' default settings.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

use crate::dwg::{self, SheetOptions};
use crate::ebook::{self, Kind};
use crate::engine::{CADCRAFT, DECKCRAFT, Engine, FILMCRAFT, GRIDCRAFT, PDFCRAFT, PHOTOCRAFT, SOUNDCRAFT, WORDCRAFT};
use crate::job::{self, Event, Item, Spec, Task};
use crate::pdf::{self, Export};

#[derive(Clone, Copy)]
pub enum Job {
    /// `<engine> convert in out`.
    Engine(Engine, &'static str),
    /// `ebook::convert`: Rust reads and writes EPUB, Kindle, text and HTML; WordCraft makes PDF and DOCX and reads
    /// word processor files. WordCraft is optional for the formats Rust handles alone.
    Ebook(Kind),
    /// `film::convert`: the output type and the export settings (`key=value`).
    Film(&'static str, &'static [&'static str]),
    PdfExport(Export),
    DwgMerged,
    DwgWhole,
}

pub struct Route {
    pub from: &'static [&'static str],
    /// What the button says.
    pub label: &'static str,
    /// The app that has more options for this conversion.
    pub app: &'static str,
    pub job: Job,
}

const DOCS: &[&str] = &["docx", "doc", "rtf", "odt"];
const SHEETS: &[&str] = &["xlsx", "csv", "tsv"];
const SLIDES: &[&str] = &["pptx", "deckcraft"];
/// Kindle files (read, never written) and the EPUB.
const KINDLE: &[&str] = &["azw3", "azw", "mobi", "prc", "kf8"];
const BOOKS: &[&str] = &["epub", "azw3", "azw", "mobi", "prc", "kf8"];
/// What becomes an EPUB: text, web pages, Markdown and word processor files.
const TO_EPUB: &[&str] = &["txt", "html", "htm", "xhtml", "md", "docx", "doc", "odt", "rtf"];
/// What FilmCraft decodes (its README): MP4/MOV, Matroska/WebM, AVI, MPEG program and transport streams, MXF, Ogg.
pub const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "ts", "mts", "m2ts", "mxf", "ogv"];
/// What SoundCraft reads (its `convert --help`). Video containers (mp4, mkv, webm) are left to the video converter.
pub const AUDIO_EXTS: &[&str] = &["wav", "wave", "bwf", "rf64", "aif", "aiff", "aifc", "flac", "mp3", "mp2", "mp1", "ogg", "oga", "m4a", "m4b", "aac", "adts", "caf"];
/// What PhotoCraft reads and writes (checked one by one with the real engine, see the `image_formats` test).
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif", "tga", "ico", "psd", "exr", "hdr", "dds", "dpx", "pnm", "ppm", "qoi"];

pub const ROUTES: &[Route] = &[
    Route { from: DOCS, label: "PDF", app: "word-to-pdf", job: Job::Engine(WORDCRAFT, "pdf") },
    Route { from: SHEETS, label: "PDF", app: "excel-to-pdf", job: Job::Engine(GRIDCRAFT, "pdf") },
    Route { from: SLIDES, label: "PDF", app: "slides-to-pdf", job: Job::Engine(DECKCRAFT, "pdf") },
    Route { from: &["pdf"], label: "Word (DOCX)", app: "pdf-tools", job: Job::PdfExport(Export::Docx) },
    Route { from: &["pdf"], label: "PowerPoint (PPTX)", app: "pdf-tools", job: Job::PdfExport(Export::Pptx) },
    Route { from: &["pdf"], label: "HTML", app: "pdf-tools", job: Job::PdfExport(Export::Html) },
    Route { from: &["pdf"], label: "RTF", app: "pdf-tools", job: Job::PdfExport(Export::Rtf) },
    Route { from: &["pdf"], label: "Text (TXT)", app: "pdf-tools", job: Job::PdfExport(Export::Txt) },
    Route { from: &["pdf"], label: "PNG pictures", app: "pdf-tools", job: Job::PdfExport(Export::Png { dpi: 150 }) },
    Route { from: &["pdf"], label: "JPG pictures", app: "pdf-tools", job: Job::PdfExport(Export::Jpeg { dpi: 150 }) },
    Route { from: IMAGE_EXTS, label: "PNG", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "png") },
    Route { from: IMAGE_EXTS, label: "JPG", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "jpg") },
    Route { from: IMAGE_EXTS, label: "WebP", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "webp") },
    Route { from: IMAGE_EXTS, label: "TIFF", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "tif") },
    Route { from: IMAGE_EXTS, label: "BMP", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "bmp") },
    Route { from: IMAGE_EXTS, label: "GIF", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "gif") },
    Route { from: IMAGE_EXTS, label: "ICO", app: "image-converter", job: Job::Engine(PHOTOCRAFT, "ico") },
    Route { from: AUDIO_EXTS, label: "WAV", app: "audio-converter", job: Job::Engine(SOUNDCRAFT, "wav") },
    Route { from: AUDIO_EXTS, label: "FLAC", app: "audio-converter", job: Job::Engine(SOUNDCRAFT, "flac") },
    Route { from: AUDIO_EXTS, label: "AIFF", app: "audio-converter", job: Job::Engine(SOUNDCRAFT, "aiff") },
    Route { from: VIDEO_EXTS, label: "MP4 (H.264)", app: "video-converter", job: Job::Film("mp4", &["format=h264", "bitrateMode=crf", "crf=23"]) },
    Route { from: VIDEO_EXTS, label: "MOV (ProRes)", app: "video-converter", job: Job::Film("mov", &["format=prores"]) },
    Route { from: VIDEO_EXTS, label: "GIF", app: "video-converter", job: Job::Film("gif", &["format=gif", "audio=false"]) },
    Route { from: VIDEO_EXTS, label: "WAV (sound)", app: "video-converter", job: Job::Film("wav", &["format=wav"]) },
    Route { from: BOOKS, label: "Text (TXT)", app: "ebook-converter", job: Job::Ebook(Kind::Txt) },
    Route { from: BOOKS, label: "HTML", app: "ebook-converter", job: Job::Ebook(Kind::Html) },
    Route { from: BOOKS, label: "PDF", app: "ebook-converter", job: Job::Ebook(Kind::Pdf) },
    Route { from: BOOKS, label: "Word (DOCX)", app: "ebook-converter", job: Job::Ebook(Kind::Docx) },
    Route { from: KINDLE, label: "EPUB", app: "ebook-converter", job: Job::Ebook(Kind::Epub) },
    Route { from: TO_EPUB, label: "EPUB", app: "ebook-converter", job: Job::Ebook(Kind::Epub) },
    Route { from: &["dwg"], label: "PDF · sheets, merged", app: "dwg-to-pdf", job: Job::DwgMerged },
    Route { from: &["dwg"], label: "PDF · whole drawing", app: "dwg-to-pdf", job: Job::DwgWhole },
];

pub fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

/// Indices of the routes that take files of this extension.
pub fn targets(ext: &str) -> Vec<usize> {
    let same = canonical(ext);
    (0..ROUTES.len()).filter(|&i| ROUTES[i].from.contains(&ext) && !is_same_format(&ROUTES[i], same)).collect()
}

/// `jpeg` is `jpg`, `tiff` is `tif`, `ppm` is `pnm`: the names one format goes by.
fn canonical(ext: &str) -> &str {
    match ext {
        "jpeg" => "jpg",
        "tiff" => "tif",
        "ppm" => "pnm",
        "aif" => "aiff",
        "wave" => "wav",
        other => other,
    }
}

/// A route that would write the format the file already is (a PNG "converted" to PNG) is not offered.
fn is_same_format(route: &Route, source: &str) -> bool {
    match route.job {
        Job::Engine(_, out) | Job::Film(out, _) => out == source,
        Job::Ebook(kind) => kind.ext() == source,
        _ => false,
    }
}

/// Every extension some route takes.
pub fn known_exts() -> Vec<&'static str> {
    let mut all: Vec<&str> = ROUTES.iter().flat_map(|r| r.from.iter().copied()).collect();
    all.sort_unstable();
    all.dedup();
    all
}

impl Job {
    fn engine(self) -> Engine {
        match self {
            Job::Ebook(_) => WORDCRAFT,
            Job::Engine(e, _) => e,
            Job::Film(..) => FILMCRAFT,
            Job::PdfExport(_) => PDFCRAFT,
            Job::DwgMerged | Job::DwgWhole => CADCRAFT,
        }
    }
}

/// Converts one file with a route, and returns the file it wrote. Used by the hub's worker thread, the
/// command line and the MCP server alike, so all three do exactly the same thing.
pub fn run_route(route: &Route, src: &Path, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    let cli = engine_for(route.job)?;
    run_with(route.job, cli, src, out_dir, note)
}

/// The engine program a job drives. An e-book job can do much without WordCraft, so for it a missing engine is
/// an empty path, and the conversion itself says when it needs the engine.
fn engine_for(job: Job) -> Result<PathBuf, String> {
    let engine = job.engine();
    match engine.find() {
        Some(p) => Ok(p),
        None if matches!(job, Job::Ebook(_)) => Ok(PathBuf::new()),
        None => Err(format!("The {} engine was not found next to this app or on this PC.", engine.folder)),
    }
}

fn run_with(job: Job, cli: PathBuf, source: &Path, out_dir: &Path, note: &dyn Fn(String)) -> Result<PathBuf, String> {
    match job {
        Job::Engine(_, out_ext) => {
            let spec = Spec { cli, out_ext: out_ext.to_owned(), extra_args: vec![], env: vec![] };
            job::convert_one_noted(&spec, source, out_dir, note)
        }
        Job::Ebook(kind) => {
            let spec = Spec { cli, out_ext: kind.ext().to_owned(), extra_args: vec![], env: vec![] };
            ebook::convert(&spec, source, out_dir)
        }
        Job::Film(out_ext, args) => {
            let spec = Spec { cli, out_ext: out_ext.to_owned(), extra_args: args.iter().map(|a| (*a).to_owned()).collect(), env: vec![] };
            crate::film::convert(&spec, source, out_dir)
        }
        Job::PdfExport(what) => pdf::export(&cli, source, what, out_dir),
        Job::DwgMerged => dwg::convert_sheets(&cli, source, out_dir, SheetOptions { include_model: false, merge: true }, note),
        Job::DwgWhole => dwg::convert_whole(&cli, source, out_dir, 420.0, true, "A3", note),
    }
}

/// Starts converting `files` (beside each source) on a worker thread. Fails at once when the route's
/// engine cannot be found.
pub fn start(route: &Route, files: Vec<PathBuf>, tx: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let cli = engine_for(route.job)?;
    let job = route.job;
    let items = files
        .into_iter()
        .map(|src| {
            let label = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let out_dir = job::out_dir_for(&job::Destination::Beside, &src);
            let (cli, source) = (cli.clone(), src.clone());
            let run: Task = Box::new(move |note| run_with(job, cli, &source, &out_dir, note));
            Item { label, source: src, run }
        })
        .collect();
    job::spawn_tasks(items, tx, cancel);
    Ok(())
}

/// A short id for a route, stable for scripts: the target format in lower case (`pdf`, `docx`, `png`,
/// `dwg-pdf-sheets`...). Two routes of one source never share an id.
pub fn route_id(route: &Route) -> String {
    match route.job {
        Job::Engine(_, ext) | Job::Film(ext, _) => ext.to_owned(),
        Job::Ebook(kind) => kind.ext().to_owned(),
        Job::PdfExport(Export::Docx) => "docx".into(),
        Job::PdfExport(Export::Pptx) => "pptx".into(),
        Job::PdfExport(Export::Html) => "html".into(),
        Job::PdfExport(Export::Rtf) => "rtf".into(),
        Job::PdfExport(Export::Txt) => "txt".into(),
        Job::PdfExport(Export::Png { .. }) => "png".into(),
        Job::PdfExport(Export::Jpeg { .. }) => "jpg".into(),
        Job::DwgMerged => "pdf-sheets".into(),
        Job::DwgWhole => "pdf-whole".into(),
    }
}

/// The route taking files of `ext` that makes `to` (a [`route_id`], or its label as a fallback).
pub fn find_route(ext: &str, to: &str) -> Option<&'static Route> {
    let to = to.trim().trim_start_matches('.').to_lowercase();
    targets(ext).into_iter().map(|i| &ROUTES[i]).find(|r| route_id(r) == to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_has_an_extension_and_a_target() {
        for r in ROUTES {
            assert!(!r.from.is_empty() && !r.label.is_empty());
        }
        assert_eq!(targets("pdf").len(), 7);
        assert_eq!(targets("dwg").len(), 2);
        assert_eq!(targets("docx").len(), 2, "pdf and epub");
        assert_eq!(targets("png").len(), 6, "every picture format but itself");
        assert_eq!(targets("jpeg").len(), 6);
        assert_eq!(targets("mp3").len(), 3, "an mp3 can become wav, flac or aiff");
        assert_eq!(targets("wav").len(), 2, "a wav is already a wav");
        assert_eq!(targets("aif").len(), 2);
        assert_eq!(targets("epub").len(), 4, "txt, html, pdf, docx");
        assert_eq!(targets("azw3").len(), 5, "and an epub");
        assert_eq!(targets("txt").len(), 1);
        assert_eq!(targets("mkv").len(), 4, "mp4, mov, gif and the sound");
        assert_eq!(targets("mp4").len(), 3, "an mp4 is already an mp4");
        assert!(!targets("jpeg").iter().any(|&i| route_id(&ROUTES[i]) == "jpg"), "a .jpeg is already a jpg");
        assert!(targets("xyz").is_empty());
    }

    #[test]
    fn route_ids_are_unique_per_source() {
        for ext in known_exts() {
            let mut ids: Vec<String> = targets(ext).into_iter().map(|i| route_id(&ROUTES[i])).collect();
            let n = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), n, "two routes of .{ext} share an id");
        }
        assert!(find_route("docx", "PDF").is_some());
        assert!(find_route("pdf", ".docx").is_some());
        assert!(find_route("pdf", "epub").is_none());
    }

    /// A 16x8 24-bit BMP, written by hand so the test needs no image library.
    fn tiny_bmp() -> Vec<u8> {
        let (w, h) = (16u32, 8u32);
        let row = (w * 3).div_ceil(4) * 4;
        let size = 54 + row * h;
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
            b.extend(std::iter::repeat_n(0u8, (row - w * 3) as usize));
        }
        b
    }

    /// Every picture format the app offers is written by the real PhotoCraft and read back (to BMP) (skipped when
    /// PhotoCraft is not installed or bundled).
    #[test]
    fn image_formats() {
        if PHOTOCRAFT.find().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vf_image_formats_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("foto città.bmp");
        std::fs::write(&src, tiny_bmp()).unwrap();
        for i in targets("bmp") {
            let route = &ROUTES[i];
            let out = run_route(route, &src, &dir, &|_| {}).unwrap_or_else(|e| panic!("bmp -> {}: {e}", route.label));
            assert!(std::fs::metadata(&out).unwrap().len() > 0, "{}", route.label);
            let back_route = find_route(&ext_of(&out), "bmp").unwrap_or_else(|| panic!("no route from {}", out.display()));
            let back = run_route(back_route, &out, &dir.join("back"), &|_| {}).unwrap_or_else(|e| panic!("{} -> bmp: {e}", route.label));
            assert!(back.is_file());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mono 16-bit 8 kHz WAV of a 440 Hz tone, written by hand.
    fn tiny_wav() -> Vec<u8> {
        let rate = 8000u32;
        let samples: Vec<i16> = (0..rate / 2).map(|n| ((n as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12000.0) as i16).collect();
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&[1, 0, 1, 0]);
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&[2, 0, 16, 0]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    /// WAV to FLAC and AIFF with the real SoundCraft, and each back to WAV (skipped without the engine).
    #[test]
    fn audio_formats() {
        if SOUNDCRAFT.find().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vf_audio_formats_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("suono città.wav");
        std::fs::write(&src, tiny_wav()).unwrap();
        for i in targets("wav") {
            let route = &ROUTES[i];
            let out = run_route(route, &src, &dir, &|_| {}).unwrap_or_else(|e| panic!("wav -> {}: {e}", route.label));
            assert!(std::fs::metadata(&out).unwrap().len() > 100, "{}", route.label);
            let back = find_route(&ext_of(&out), "wav").unwrap();
            let again = run_route(back, &out, &dir.join("back"), &|_| {}).unwrap_or_else(|e| panic!("{} -> wav: {e}", route.label));
            assert!(again.is_file());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extensions_are_lower_case() {
        assert_eq!(ext_of(Path::new("A.DOCX")), "docx");
        assert_eq!(ext_of(Path::new("noext")), "");
    }
}
