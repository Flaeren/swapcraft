//! The apps the hub lists. Ready apps point at a portable build; the rest are planned converters
//! (formats still to be verified, see TODO.md).

use std::path::PathBuf;

pub struct App {
    pub id: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    pub blurb: &'static str,
    /// The ArtCraft engine doing the work.
    pub engine: &'static str,
    pub color: [u8; 3],
    /// Path of the exe inside a portable build: `<folder>/<exe>`. None while only planned.
    pub exe: Option<&'static str>,
    /// Whether a downloadable pack exists for it (the converters that are built, not the planned ones).
    pub pack: bool,
}

pub const APPS: &[App] = &[
    App {
        id: "dwg-to-pdf",
        name: "DWG to PDF",
        tagline: "CAD drawings to PDF",
        blurb: "Sheets or the whole drawing, merged or separate. Runs offline: drawings never leave this PC.",
        engine: "CADCraft",
        color: [0x3f, 0xd6, 0xb8],
        exe: Some("DWG2PDF-portable/DWG2PDF.exe"),
        pack: true,
    },
    App {
        id: "word-to-pdf",
        name: "Word to PDF",
        tagline: "Documents to PDF",
        blurb: "DOCX, DOC, RTF and ODT to PDF, one file or a whole folder. No Office needed, nothing uploaded.",
        engine: "WordCraft",
        color: [0x2d, 0x81, 0xff],
        exe: Some("WORD2PDF-portable/WORD2PDF.exe"),
        pack: true,
    },
    App {
        id: "excel-to-pdf",
        name: "Excel to PDF",
        tagline: "Spreadsheets to PDF",
        blurb: "XLSX, CSV and TSV spreadsheets to PDF, one file or a whole folder. No Office needed, nothing uploaded.",
        engine: "GridCraft",
        color: [0x4a, 0xd2, 0x7a],
        exe: None,
        pack: true,
    },
    App {
        id: "slides-to-pdf",
        name: "Slides to PDF",
        tagline: "Presentations to PDF",
        blurb: "PowerPoint presentations to PDF, one file or a whole folder. No Office needed, nothing uploaded.",
        engine: "DeckCraft",
        color: [0xff, 0xc1, 0x4d],
        exe: None,
        pack: true,
    },
    App {
        id: "pdf-tools",
        name: "PDF Tools",
        tagline: "Convert, merge, split, OCR, shrink",
        blurb: "PDF to Word, PowerPoint, HTML, RTF, text or pictures; merge, split, extract pages, shrink; OCR in English.",
        engine: "PdfCraft",
        color: [0xff, 0x62, 0x59],
        exe: None,
        pack: true,
    },
    App {
        id: "image-converter",
        name: "Image Converter",
        tagline: "Convert images in batch",
        blurb: "PNG, JPG, WebP, TIFF and more, one image or a folder.",
        engine: "PhotoCraft",
        color: [0xb0, 0x8c, 0xff],
        exe: None,
        pack: false,
    },
    App {
        id: "audio-converter",
        name: "Audio Converter",
        tagline: "Convert and normalise audio",
        blurb: "Change format, sample rate and bit depth; normalise levels.",
        engine: "SoundCraft",
        color: [0xff, 0x7a, 0xc8],
        exe: None,
        pack: false,
    },
    App {
        id: "video-converter",
        name: "Video Converter",
        tagline: "Convert and compress video",
        blurb: "Re-encode or shrink video to the format you need.",
        engine: "FilmCraft",
        color: [0x4d, 0xc8, 0xff],
        exe: None,
        pack: false,
    },
];

/// Looks for the portable build next to the hub (`apps/<folder>/<exe>`), or, in the source tree,
/// in the app's own `dist` folder.
pub fn locate(app: &App) -> Option<PathBuf> {
    let here = std::env::current_exe().ok()?;
    let rust_exe = format!("{}.exe", app.id);
    for base in here.parent()?.ancestors() {
        // Rust apps: `apps/<id>/<id>.exe` (or, in the source tree, the cargo release folder).
        let mut candidates = vec![base.join("apps").join(app.id).join(&rust_exe), base.join("target").join("release").join(&rust_exe)];
        // Python apps: `apps/<folder>/<exe>` or the app's own `dist` folder.
        if let Some(rel) = app.exe {
            candidates.push(base.join("apps").join(rel));
            candidates.push(base.join("legacy").join(app.id).join("dist").join(rel));
        }
        if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
            return Some(found);
        }
    }
    None
}
