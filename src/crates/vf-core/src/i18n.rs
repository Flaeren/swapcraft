//! Texts in English (the source language) plus Italian, Spanish, French and German.
//!
//! Catalogs are TSV files embedded in the exe: a header `key<TAB>en<TAB>it<TAB>es<TAB>fr<TAB>de`, then one
//! row per text. `\n` in a cell is a line break. A missing translation falls back to English, a missing
//! key to the key itself, so a gap is visible but never a crash.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    It,
    Es,
    Fr,
    De,
}

impl Lang {
    pub const ALL: [Lang; 5] = [Lang::En, Lang::It, Lang::Es, Lang::Fr, Lang::De];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::It => "it",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::De => "de",
        }
    }

    /// The language's own name, as shown in the language menu.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::It => "Italiano",
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.trim().to_lowercase();
        let primary = primary.split(['-', '_']).next().unwrap_or("");
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

    fn column(self) -> usize {
        Lang::ALL.iter().position(|l| *l == self).unwrap_or(0)
    }
}

struct State {
    lang: Lang,
    /// key -> [en, it, es, fr, de]; empty cell = not translated.
    rows: HashMap<String, [String; 5]>,
}

fn state() -> &'static RwLock<State> {
    static STATE: OnceLock<RwLock<State>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(State { lang: Lang::En, rows: HashMap::new() }))
}

/// `%APPDATA%\voidflare\language.txt`: the choice is shared by every voidflare app, like the Python ones.
fn remembered_path() -> Option<PathBuf> {
    crate::platform::data_file("language.txt")
}

/// The language to use: the `VOIDFLARE_LANG` override, else what the user chose (the settings of the hub), else
/// English. The language of Windows is deliberately not consulted: English is the base, anything else is a choice.
pub fn choose(env: Option<&str>, remembered: Option<&str>) -> Lang {
    env.and_then(Lang::from_code).or_else(|| remembered.and_then(Lang::from_code)).unwrap_or(Lang::En)
}

/// The language the user chose and saved, if any.
pub fn remembered() -> Option<Lang> {
    remembered_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|c| Lang::from_code(&c))
}

/// Loads catalogs and picks the language (see [`choose`]).
pub fn init(catalogs: &[&str]) {
    let mut s = state().write().unwrap();
    for text in catalogs {
        add_catalog(&mut s.rows, text);
    }
    let env = std::env::var("VOIDFLARE_LANG").ok();
    let saved = remembered_path().and_then(|p| std::fs::read_to_string(p).ok());
    s.lang = choose(env.as_deref(), saved.as_deref());
}

fn add_catalog(rows: &mut HashMap<String, [String; 5]>, text: &str) {
    for line in text.lines().skip(1) {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cells = line.split('\t').map(|c| c.replace("\\n", "\n"));
        let Some(key) = cells.next() else { continue };
        let mut row: [String; 5] = Default::default();
        for (slot, cell) in row.iter_mut().zip(cells) {
            *slot = cell;
        }
        rows.insert(key, row);
    }
}

pub fn language() -> Lang {
    state().read().unwrap().lang
}

pub fn set_language(lang: Lang, remember: bool) {
    state().write().unwrap().lang = lang;
    if remember && let Some(p) = remembered_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, lang.code());
    }
}

/// The text for `key` in the current language. On Linux a row `key@linux` wins over `key` when the catalog has
/// one: the few texts that name Windows things (the Start menu, File Explorer) have a Linux wording there.
pub fn t(key: &str) -> String {
    let s = state().read().unwrap();
    let linux = (!cfg!(windows)).then(|| format!("{key}@linux"));
    let row = linux.as_deref().and_then(|k| s.rows.get(k)).or_else(|| s.rows.get(key));
    match row {
        Some(row) => {
            let own = &row[s.lang.column()];
            if own.is_empty() { row[0].clone() } else { own.clone() }
        }
        None => key.to_owned(),
    }
}

/// Like [`t`], replacing each `{name}` with its value.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut out = t(key);
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Keys that are missing in some language, for tests: `(key, language code)`.
pub fn gaps(catalog: &str) -> Vec<(String, &'static str)> {
    let mut rows = HashMap::new();
    add_catalog(&mut rows, catalog);
    let mut out = Vec::new();
    for (key, row) in rows {
        for (i, lang) in Lang::ALL.iter().enumerate() {
            if row[i].is_empty() {
                out.push((key.clone(), lang.code()));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_parse() {
        assert_eq!(Lang::from_code("it-IT"), Some(Lang::It));
        assert_eq!(Lang::from_code("de_AT"), Some(Lang::De));
        assert_eq!(Lang::from_code("pt-BR"), None);
    }

    #[test]
    fn english_unless_chosen() {
        assert_eq!(choose(None, None), Lang::En);
        assert_eq!(choose(None, Some("it")), Lang::It);
        assert_eq!(choose(Some("de"), Some("it")), Lang::De);
        // an unknown or unsupported code is ignored, never an error
        assert_eq!(choose(Some("xx"), Some("pt-BR")), Lang::En);
    }

    #[test]
    fn lookup_falls_back_to_english_then_key() {
        init(&["key\ten\tit\tes\tfr\tde\nhello\tHello\tCiao\t\t\t\nfmt\tCopy {n} files\t\t\t\t"]);
        set_language(Lang::It, false);
        assert_eq!(t("hello"), "Ciao");
        set_language(Lang::Es, false);
        assert_eq!(t("hello"), "Hello");
        assert_eq!(tf("fmt", &[("n", "3")]), "Copy 3 files");
        assert_eq!(t("nope"), "nope");
    }
}
