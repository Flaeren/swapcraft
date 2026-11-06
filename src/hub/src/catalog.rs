//! The apps the hub lists. Ready apps point at a portable build; the rest are planned converters
//! (formats still to be verified, see TODO.md).

use std::path::PathBuf;

use vf_core::i18n;

pub struct App {
    pub id: &'static str,
    pub name: &'static str,
    /// The ArtCraft engine doing the work.
    pub engine: &'static str,
    pub color: [u8; 3],
    /// Path of the exe inside a portable build: `<folder>/<exe>`. None while only planned.
    pub exe: Option<&'static str>,
    /// Whether a downloadable pack exists for it (the converters that are built, not the planned ones).
    pub pack: bool,
}

impl App {
    /// The one-line description under the name, in the chosen language.
    pub fn tagline(&self) -> String {
        i18n::t(&format!("hub.app.{}.tagline", self.id))
    }

    /// The short paragraph on the card, in the chosen language.
    pub fn blurb(&self) -> String {
        i18n::t(&format!("hub.app.{}.blurb", self.id))
    }
}

pub const APPS: &[App] = &[
    App {
        id: "dwg-to-pdf",
        name: "DWG to PDF",
        engine: "CADCraft",
        color: [0x3f, 0xd6, 0xb8],
        exe: Some("DWG2PDF-portable/DWG2PDF.exe"),
        pack: true,
    },
    App {
        id: "word-to-pdf",
        name: "Word to PDF",
        engine: "WordCraft",
        color: [0x2d, 0x81, 0xff],
        exe: Some("WORD2PDF-portable/WORD2PDF.exe"),
        pack: true,
    },
    App {
        id: "excel-to-pdf",
        name: "Excel to PDF",
        engine: "GridCraft",
        color: [0x4a, 0xd2, 0x7a],
        exe: None,
        pack: true,
    },
    App {
        id: "slides-to-pdf",
        name: "Slides to PDF",
        engine: "DeckCraft",
        color: [0xff, 0xc1, 0x4d],
        exe: None,
        pack: true,
    },
    App {
        id: "pdf-tools",
        name: "PDF Tools",
        engine: "PdfCraft",
        color: [0xff, 0x62, 0x59],
        exe: None,
        pack: true,
    },
    App {
        id: "image-converter",
        name: "Image Converter",
        engine: "PhotoCraft",
        color: [0xb0, 0x8c, 0xff],
        exe: None,
        pack: true,
    },
    App {
        id: "audio-converter",
        name: "Audio Converter",
        engine: "SoundCraft",
        color: [0xff, 0x7a, 0xc8],
        exe: None,
        pack: true,
    },
    App {
        id: "video-converter",
        name: "Video Converter",
        engine: "FilmCraft",
        color: [0x4d, 0xc8, 0xff],
        exe: None,
        pack: true,
    },
    App {
        id: "ebook-converter",
        name: "E-book Converter",
        engine: "WordCraft",
        color: [0xe8, 0xb0, 0x5a],
        exe: None,
        pack: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = include_str!("../assets/hub.tsv");

    /// Every key the hub's code asks for, found in the sources, must be in the catalog in all five languages,
    /// and every catalog key must be used (so a removed text does not linger untranslated).
    #[test]
    fn the_hub_catalog_is_complete_and_matches_the_code() {
        assert!(i18n::gaps(CATALOG).is_empty(), "missing translations: {:?}", i18n::gaps(CATALOG));
        let keys: Vec<&str> = CATALOG.lines().skip(1).filter_map(|l| l.split('\t').next()).collect();
        // catalog.rs last: the split('"') below has one quote, which would shift every file after it
        let sources = [include_str!("main.rs"), include_str!("settings.rs"), include_str!("recipes.rs"), include_str!("menu.rs"), include_str!("catalog.rs")].join("\n");
        let mut used: Vec<String> = Vec::new();
        for part in sources.split('"').skip(1).step_by(2) {
            if ["hub.", "rec.", "menu."].iter().any(|p| part.starts_with(p)) && !part.ends_with(".rs") && !part.ends_with('.') && part.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
                used.push(part.to_owned());
            }
        }
        // built with format! in recipes.rs
        for kind in vf_core::recipe::Step::KINDS {
            used.push(format!("rec.step_{kind}"));
        }
        for level in vf_core::recipe::Level::ALL {
            used.push(format!("rec.level_{}", level.word()));
        }
        for app in APPS {
            used.push(format!("hub.app.{}.tagline", app.id));
            used.push(format!("hub.app.{}.blurb", app.id));
        }
        for key in &used {
            // the app keys are built with format!, so the placeholder form is not a real key
            if key.contains('{') {
                continue;
            }
            assert!(keys.contains(&key.as_str()), "the code asks for {key}, which is not in hub.tsv");
        }
        for key in keys {
            assert!(used.iter().any(|u| u == key), "{key} is in hub.tsv but no code uses it");
        }
    }
}
