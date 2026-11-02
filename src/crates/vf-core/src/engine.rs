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
pub const PHOTOCRAFT: Engine = Engine { folder: "PhotoCraft", cli: "photocraft-cli.exe", env: "VOIDFLARE_PHOTOCRAFT" };
pub const SOUNDCRAFT: Engine = Engine { folder: "SoundCraft", cli: "soundcraft-cli.exe", env: "VOIDFLARE_SOUNDCRAFT" };
pub const FILMCRAFT: Engine = Engine { folder: "FilmCraft", cli: "filmcraft-cli.exe", env: "VOIDFLARE_FILMCRAFT" };
/// CADCraft installs under `%LOCALAPPDATA%ProgramsArtCraft`, not Program Files (`find` looks in both).
pub const CADCRAFT: Engine = Engine { folder: "CADCraft", cli: "cadcraft-cli.exe", env: "VOIDFLARE_CADCRAFT" };

/// The engine a manifest names (`wordcraft`, `gridcraft`, ...), if voidflare drives it.
pub fn by_name(name: &str) -> Option<&'static Engine> {
    [&WORDCRAFT, &GRIDCRAFT, &DECKCRAFT, &PDFCRAFT, &CADCRAFT, &PHOTOCRAFT, &SOUNDCRAFT, &FILMCRAFT].into_iter().find(|e| e.folder.eq_ignore_ascii_case(name))
}

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
        candidates.extend(self.system_candidates());
        candidates.into_iter().find(|c| c.is_file())
    }

    fn system_candidates(&self) -> Vec<PathBuf> {
        let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let mut list = vec![program_files.join(self.folder).join(self.cli)];
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            list.push(PathBuf::from(local).join("Programs").join("ArtCraft").join(self.folder).join(self.cli));
        }
        list
    }

    /// Only the ArtCraft install on this PC (Program Files, `%LOCALAPPDATA%\Programs\ArtCraft`): not the override
    /// and not a copy bundled next to an app. This is what a pack can reuse instead of carrying its own engine.
    pub fn find_system(&self) -> Option<PathBuf> {
        self.system_candidates().into_iter().find(|c| c.is_file())
    }
}

/// The version number in an engine's `--version` line: `wordcraft-cli 0.13.2` gives `0.13.2`,
/// `photocraft-cli 0.18.0 (8a87a457a, 2026-10-31)` gives `0.18.0`.
pub fn parse_version(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|word| {
        let v: String = word.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let v = v.trim_end_matches('.');
        (v.split('.').count() >= 2 && v.split('.').all(|p| !p.is_empty())).then(|| v.to_owned())
    })
}

/// Asks an engine program for its version (`--version`), with no window and a five-second limit.
pub fn version_of(cli: &Path) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(cli);
    cmd.arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None if started.elapsed() > std::time::Duration::from_secs(5) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    out.lines().find_map(parse_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_the_version_line() {
        assert_eq!(parse_version("wordcraft-cli 0.13.2").as_deref(), Some("0.13.2"));
        assert_eq!(parse_version("photocraft-cli 0.18.0 (8a87a457a, 2026-10-31)").as_deref(), Some("0.18.0"));
        assert_eq!(parse_version("cadcraft-cli 0.14.0 (dev)").as_deref(), Some("0.14.0"));
        assert_eq!(parse_version("soundcraft-cli 0.15.0 (windows x86_64)").as_deref(), Some("0.15.0"));
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("x 12"), None);
    }
}
