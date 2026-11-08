//! `%APPDATA%\voidflare\dragmenu.tsv`: what the drag-and-drop menu offers, written by the hub when the right-click
//! menu is added (the same entries, from `vf_core::menu::entries`). The DLL only reads it, so it needs none of the
//! conversion code and nothing heavy is loaded into Explorer.
//!
//! ```text
//! voidflare-dragmenu 1
//! exe	C:\…\voidflare.exe
//! title	Convert here with voidflare
//! e	dwg	route:pdf-sheets	0	PDF · sheets, merged
//! e	dwg	recipe:Drawings for the client	1	Recipe: Drawings for the client
//! ```
//!
//! `e` lines: extension (lower case, no dot), action (as `vf_core::menu::Action::encode`), 1 for a line above it,
//! label. Tabs and line breaks never appear inside a field: the writer replaces them with spaces.

use std::path::{Path, PathBuf};

pub const HEADER: &str = "voidflare-dragmenu 1";
/// The handler's class id as the registry writes it (the same as `handler::CLSID`, checked by a test).
pub const CLSID_STR: &str = "{127B4C5D-6B1D-489C-A1D9-1ABF98D0C829}";
/// The DLL's file name, next to `voidflare.exe`.
pub const DLL_NAME: &str = "voidflare_dragdrop.dll";
pub const FILE_NAME: &str = "dragmenu.tsv";

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub action: String,
    pub separator: bool,
    pub label: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Config {
    pub exe: PathBuf,
    pub title: String,
    /// (extension, entries in menu order)
    pub types: Vec<(String, Vec<Entry>)>,
}

/// A field as written: no tabs or line breaks inside.
pub fn field(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ")
}

impl Config {
    pub fn parse(text: &str) -> Option<Config> {
        let mut lines = text.lines();
        if lines.next()?.trim_start_matches('\u{feff}').trim() != HEADER {
            return None;
        }
        let mut c = Config::default();
        for line in lines {
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.as_slice() {
                ["exe", p] => c.exe = PathBuf::from(p),
                ["title", t] => c.title = (*t).to_owned(),
                ["e", ext, action, sep, label] => {
                    let entry = Entry { action: (*action).to_owned(), separator: *sep == "1", label: (*label).to_owned() };
                    let ext = ext.to_ascii_lowercase();
                    match c.types.iter_mut().find(|(e, _)| *e == ext) {
                        Some((_, list)) => list.push(entry),
                        None => c.types.push((ext, vec![entry])),
                    }
                }
                _ => {}
            }
        }
        (!c.exe.as_os_str().is_empty() && !c.title.is_empty()).then_some(c)
    }

    pub fn write(&self) -> String {
        let mut out = format!("{HEADER}\nexe\t{}\ntitle\t{}\n", field(&self.exe.to_string_lossy()), field(&self.title));
        for (ext, list) in &self.types {
            for e in list {
                out += &format!("e\t{}\t{}\t{}\t{}\n", field(ext), field(&e.action), if e.separator { 1 } else { 0 }, field(&e.label));
            }
        }
        out
    }

    /// The entries that fit every one of `files`: those of the first file's type whose action every other type has
    /// too. Empty when a file has no entries (a folder, a type voidflare does not read) or there are no files.
    pub fn entries_for(&self, files: &[PathBuf]) -> Vec<Entry> {
        let mut exts: Vec<String> = files.iter().map(|f| ext_of(f)).collect();
        exts.dedup();
        let lists: Option<Vec<&Vec<Entry>>> = exts.iter().map(|e| self.types.iter().find(|(x, _)| x == e).map(|(_, l)| l)).collect();
        let Some(lists) = lists.filter(|l| !l.is_empty()) else { return Vec::new() };
        let mut out: Vec<Entry> = lists[0].iter().filter(|e| lists[1..].iter().all(|l| l.iter().any(|o| o.action == e.action))).cloned().collect();
        // No line at the very top, and none twice in a row after entries were dropped.
        if let Some(e) = out.first_mut() {
            e.separator = false;
        }
        out
    }
}

pub fn ext_of(p: &Path) -> String {
    if p.is_dir() {
        return String::new();
    }
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// `%APPDATA%\voidflare\dragmenu.tsv`
pub fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("voidflare").join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        let e = |a: &str, s: bool, l: &str| Entry { action: a.into(), separator: s, label: l.into() };
        Config {
            exe: PathBuf::from(r"C:\v f\voidflare.exe"),
            title: "Convert here with voidflare".into(),
            types: vec![
                ("docx".into(), vec![e("route:pdf", false, "PDF"), e("route:txt", false, "Text"), e("recipe:Report pack", true, "Recipe: Report pack"), e("app:word-to-pdf", true, "Open in Word to PDF…")]),
                ("xlsx".into(), vec![e("route:pdf", false, "PDF"), e("recipe:Report pack", true, "Recipe: Report pack"), e("app:excel-to-pdf", true, "Open in Excel to PDF…")]),
            ],
        }
    }

    #[test]
    fn round_trips_and_cleans_fields() {
        let mut c = sample();
        c.title = "a\tb\nc".into();
        let back = Config::parse(&c.write()).unwrap();
        assert_eq!(back.title, "a b c");
        assert_eq!(back.types, sample().types);
        assert_eq!(back.exe, sample().exe);
    }

    #[test]
    fn refuses_other_files() {
        assert!(Config::parse("something else\nexe\tx\ntitle\ty\n").is_none());
        assert!(Config::parse(&format!("{HEADER}\ntitle\ty\n")).is_none(), "no exe");
    }

    #[test]
    fn mixed_types_keep_only_what_all_of_them_have() {
        let c = sample();
        let one = c.entries_for(&[PathBuf::from("a.docx"), PathBuf::from("b.DOCX")]);
        assert_eq!(one.len(), 4);
        let both = c.entries_for(&[PathBuf::from("a.docx"), PathBuf::from("b.xlsx")]);
        let actions: Vec<&str> = both.iter().map(|e| e.action.as_str()).collect();
        assert_eq!(actions, ["route:pdf", "recipe:Report pack"]);
        assert!(!both[0].separator);
        assert!(c.entries_for(&[PathBuf::from("a.docx"), PathBuf::from("b.zip")]).is_empty());
        assert!(c.entries_for(&[]).is_empty());
    }
}
