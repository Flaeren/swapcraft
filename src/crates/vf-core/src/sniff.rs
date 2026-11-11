//! What a file is when its name does not say: Linux text editors and many programs save files with no extension
//! ("notes", "Untitled document"), and the routes, the apps and the engines all go by the extension. A look at the
//! first bytes gives the kind for the common formats; anything unsure is left unknown rather than guessed.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// How much of the file is read: enough for a ZIP's first entry names and a fair sample of text.
const SAMPLE: u64 = 64 * 1024;

/// The file's extension in lower case, or, when it has none, the kind its content shows (`txt`, `pdf`, `docx`...).
/// Empty when neither says anything.
pub fn ext_or_kind(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    if !ext.is_empty() || !path.is_file() {
        return ext;
    }
    kind(path).unwrap_or_default().to_owned()
}

/// The kind of file the content shows, as the extension it would have.
pub fn kind(path: &Path) -> Option<&'static str> {
    let mut head = Vec::new();
    fs::File::open(path).ok()?.take(SAMPLE).read_to_end(&mut head).ok()?;
    kind_of(&head)
}

fn kind_of(b: &[u8]) -> Option<&'static str> {
    let at = |i: usize, sig: &[u8]| b.len() >= i + sig.len() && &b[i..i + sig.len()] == sig;
    let has = |needle: &[u8]| b.windows(needle.len()).any(|w| w == needle);
    if b.is_empty() {
        return None;
    }
    if at(0, b"%PDF-") {
        return Some("pdf");
    }
    if at(0, b"AC10") {
        return Some("dwg");
    }
    if at(0, b"PK\x03\x04") {
        // an EPUB and an OpenDocument file name their type in the first entry; Office files by their folders
        return if has(b"mimetypeapplication/epub+zip") {
            Some("epub")
        } else if has(b"application/vnd.oasis.opendocument.text") {
            Some("odt")
        } else if has(b"word/") {
            Some("docx")
        } else if has(b"xl/") {
            Some("xlsx")
        } else if has(b"ppt/") {
            Some("pptx")
        } else {
            None
        };
    }
    if at(0, b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if at(0, b"\xFF\xD8\xFF") {
        return Some("jpg");
    }
    if at(0, b"GIF87a") || at(0, b"GIF89a") {
        return Some("gif");
    }
    if at(0, b"II*\0") || at(0, b"MM\0*") {
        return Some("tif");
    }
    if at(0, b"8BPS") {
        return Some("psd");
    }
    if at(0, b"BM") && b.len() > 26 && at(6, b"\0\0\0\0") {
        return Some("bmp");
    }
    if at(0, b"RIFF") {
        return if at(8, b"WAVE") {
            Some("wav")
        } else if at(8, b"WEBP") {
            Some("webp")
        } else if at(8, b"AVI ") {
            Some("avi")
        } else {
            None
        };
    }
    if at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        return Some("aiff");
    }
    if at(0, b"fLaC") {
        return Some("flac");
    }
    if at(0, b"ID3") {
        return Some("mp3");
    }
    if at(0, b"OggS") {
        return Some("ogg");
    }
    if at(4, b"ftyp") {
        return if at(8, b"M4A ") { Some("m4a") } else { Some("mp4") };
    }
    if at(0, b"\x1A\x45\xDF\xA3") {
        return Some("mkv");
    }
    if is_text(b) {
        let start = String::from_utf8_lossy(&b[..b.len().min(512)]).trim_start_matches('\u{feff}').trim_start().to_lowercase();
        return Some(if start.starts_with("{\\rtf") {
            "rtf"
        } else if start.starts_with("<!doctype html") || start.starts_with("<html") {
            "html"
        } else {
            "txt"
        });
    }
    None
}

/// Text: no NUL and no control characters but tab, line ends and form feed. UTF-8 or a one-byte code page alike.
fn is_text(b: &[u8]) -> bool {
    !b.iter().any(|&c| c < 0x09 || (0x0e..0x20).contains(&c) && c != 0x1b || c == 0x7f)
}

/// Runs `convert` on `src`, or, when `src` has no extension but its content shows the kind, on a copy named
/// `<name>.<kind>` in a scratch folder: the engines choose how to read a file by its extension. The output keeps
/// the original name, since the copy has the same stem.
pub fn with_extension<T>(src: &Path, convert: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
    if src.extension().is_some() {
        return convert(src);
    }
    let Some(kind) = kind(src) else { return convert(src) };
    let tmp = crate::job::scratch_dir().map_err(|e| e.to_string())?;
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let copy: PathBuf = tmp.join(format!("{name}.{kind}"));
    let result = fs::copy(src, &copy).map_err(|e| e.to_string()).and_then(|_| convert(&copy));
    let _ = fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_content() {
        assert_eq!(kind_of(b"Shopping list\n- milk\n- bread\n"), Some("txt"));
        assert_eq!(kind_of("Lista della spesa: caffè, però\r\n".as_bytes()), Some("txt"));
        assert_eq!(kind_of(b"\xEF\xBB\xBF  <!DOCTYPE html><html><body>Hi</body></html>"), Some("html"));
        assert_eq!(kind_of(b"{\\rtf1\\ansi Hello}"), Some("rtf"));
        assert_eq!(kind_of(b"%PDF-1.7\n..."), Some("pdf"));
        assert_eq!(kind_of(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
        assert_eq!(kind_of(b"RIFF\x24\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(kind_of(b"PK\x03\x04\x14\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0mimetypeapplication/epub+zip"), Some("epub"));
        assert_eq!(kind_of(b"PK\x03\x04....[Content_Types].xml....word/document.xml"), Some("docx"));
        assert_eq!(kind_of(b"\0\0\0\x20ftypisom"), Some("mp4"));
        // a program, a ZIP of something else, nothing at all: unknown, not guessed
        assert_eq!(kind_of(b"\x7fELF\x02\x01\x01\0\0\0\0\0"), None);
        assert_eq!(kind_of(b"PK\x03\x04....photos/a.jpg"), None);
        assert_eq!(kind_of(b""), None);
    }

    #[test]
    fn a_file_without_extension_converts_through_a_named_copy() {
        let dir = crate::job::scratch_dir().unwrap();
        let plain = dir.join("notes");
        fs::write(&plain, "Some notes\n").unwrap();
        assert_eq!(ext_or_kind(&plain), "txt");
        let seen = with_extension(&plain, |p| Ok(p.file_name().unwrap().to_string_lossy().into_owned())).unwrap();
        assert_eq!(seen, "notes.txt");
        let named = dir.join("a.md");
        fs::write(&named, "# A\n").unwrap();
        assert_eq!(ext_or_kind(&named), "md");
        assert_eq!(with_extension(&named, |p| Ok(p.to_path_buf())).unwrap(), named);
        let _ = fs::remove_dir_all(&dir);
    }
}
