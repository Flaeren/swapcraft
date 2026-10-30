//! Finding an installed ArtCraft engine (its `*-cli.exe`).

use std::path::{Path, PathBuf};

/// One ArtCraft engine: the folder it installs to and its command-line program.
#[derive(Clone, Copy, Debug)]
pub struct Engine {
    /// Folder name, e.g. `WordCraft`.
    pub folder: &'static str,
    /// Program name, e.g. `wordcraft-cli.exe`.
    pub cli: &'static str,
    /// Environment variable that points at a program to use instead (to compare engines).
    pub env: &'static str,
}

pub const WORDCRAFT: Engine = Engine { folder: "WordCraft", cli: "wordcraft-cli.exe", env: "VOIDFLARE_WORDCRAFT" };
pub const GRIDCRAFT: Engine = Engine { folder: "GridCraft", cli: "gridcraft-cli.exe", env: "VOIDFLARE_GRIDCRAFT" };
pub const DECKCRAFT: Engine = Engine { folder: "DeckCraft", cli: "deckcraft-cli.exe", env: "VOIDFLARE_DECKCRAFT" };
pub const PDFCRAFT: Engine = Engine { folder: "PdfCraft", cli: "pdfcraft-cli.exe", env: "VOIDFLARE_PDFCRAFT" };
/// CADCraft installs under `%LOCALAPPDATA%ProgramsArtCraft`, not Program Files (`find` looks in both).
pub const CADCRAFT: Engine = Engine { folder: "CADCraft", cli: "cadcraft-cli.exe", env: "VOIDFLARE_CADCRAFT" };

impl Engine {
    /// The override, then a copy bundled next to the app (`<app dir>\<folder lower-case>\`), then the
    /// installed engine in Program Files, then in `%LOCALAPPDATA%\Programs\ArtCraft`.
    pub fn find(&self) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = std::env::var_os(self.env) {
            candidates.push(PathBuf::from(p));
        }
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            candidates.push(dir.join(self.folder.to_lowercase()).join(self.cli));
            // The hub sits above the apps: `apps\<app>\<engine>\`.
            if let Ok(apps) = std::fs::read_dir(dir.join("apps")) {
                for app in apps.flatten() {
                    candidates.push(app.path().join(self.folder.to_lowercase()).join(self.cli));
                }
            }
        }
        let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        candidates.push(program_files.join(self.folder).join(self.cli));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join("Programs").join("ArtCraft").join(self.folder).join(self.cli));
        }
        candidates.into_iter().find(|c| c.is_file())
    }
}
