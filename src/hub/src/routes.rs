//! What the hub can convert: every route says which extensions it takes, what it makes and which engine
//! does it. The drop panel offers the routes matching the dropped file. Merge, split, OCR and the other
//! options stay in the apps; routes run with the apps' default settings.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

use vf_core::dwg::{self, SheetOptions};
use vf_core::engine::{CADCRAFT, DECKCRAFT, Engine, GRIDCRAFT, PDFCRAFT, WORDCRAFT};
use vf_core::job::{self, Event, Item, Spec, Task};
use vf_core::pdf::{self, Export};

#[derive(Clone, Copy)]
pub enum Job {
    /// `<engine> convert in out`.
    Engine(Engine, &'static str),
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
    Route { from: &["dwg"], label: "PDF · sheets, merged", app: "dwg-to-pdf", job: Job::DwgMerged },
    Route { from: &["dwg"], label: "PDF · whole drawing", app: "dwg-to-pdf", job: Job::DwgWhole },
];

pub fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

/// Indices of the routes that take files of this extension.
pub fn targets(ext: &str) -> Vec<usize> {
    (0..ROUTES.len()).filter(|&i| ROUTES[i].from.contains(&ext)).collect()
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
            Job::Engine(e, _) => e,
            Job::PdfExport(_) => PDFCRAFT,
            Job::DwgMerged | Job::DwgWhole => CADCRAFT,
        }
    }
}

/// Starts converting `files` (beside each source) on a worker thread. Fails at once when the route's
/// engine cannot be found.
pub fn start(route: &Route, files: Vec<PathBuf>, tx: mpsc::Sender<Event>, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let engine = route.job.engine();
    let cli = engine.find().ok_or_else(|| format!("The {} engine was not found next to this app or on this PC.", engine.folder))?;
    let job = route.job;
    let items = files
        .into_iter()
        .map(|src| {
            let label = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let out_dir = job::out_dir_for(&job::Destination::Beside, &src);
            let (cli, source) = (cli.clone(), src.clone());
            let run: Task = match job {
                Job::Engine(_, out_ext) => {
                    let spec = Spec { cli, out_ext: out_ext.to_owned(), extra_args: vec![], env: vec![] };
                    Box::new(move |_| job::convert_one(&spec, &source, &out_dir))
                }
                Job::PdfExport(what) => Box::new(move |_| pdf::export(&cli, &source, what, &out_dir)),
                Job::DwgMerged => Box::new(move |note| {
                    dwg::convert_sheets(&cli, &source, &out_dir, SheetOptions { include_model: false, merge: true }, note)
                }),
                Job::DwgWhole => Box::new(move |note| dwg::convert_whole(&cli, &source, &out_dir, 420.0, true, "A3", note)),
            };
            Item { label, source: src, run }
        })
        .collect();
    job::spawn_tasks(items, tx, cancel);
    Ok(())
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
        assert_eq!(targets("docx").len(), 1);
        assert!(targets("xyz").is_empty());
    }

    #[test]
    fn extensions_are_lower_case() {
        assert_eq!(ext_of(Path::new("A.DOCX")), "docx");
        assert_eq!(ext_of(Path::new("noext")), "");
    }
}
