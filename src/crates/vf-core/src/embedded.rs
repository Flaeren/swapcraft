//! Documents embedded in a Word file. Word shows an embedded object (a PDF, a Word, Excel or PowerPoint file
//! pasted in with *Insert > Object*) as a small picture; the engine draws that picture and the document inside
//! is lost. This reads the objects out of the `.docx` so their pages can follow the converted PDF.
//!
//! An object is stored under `word/embeddings/` either as the file itself (`.docx`, `.xlsx`, `.pptx`, `.pdf`) or
//! wrapped in an OLE container (`.bin`): Acrobat keeps its PDF in the `CONTENTS` stream, Office in `Package`, and
//! other programs in `Ole10Native`. Old binary formats (a `.doc` or `.xls` stored as a plain OLE file) are not
//! readable here and are counted, not guessed at.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use crate::engine::{DECKCRAFT, GRIDCRAFT};
use crate::i18n::tf;
use crate::job::{Spec, run_command, scratch_dir};
use crate::pdfmerge::merge_with_bookmarks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Word,
    Excel,
    Slides,
}

impl Kind {
    fn ext(self) -> &'static str {
        match self {
            Kind::Pdf => "pdf",
            Kind::Word => "docx",
            Kind::Excel => "xlsx",
            Kind::Slides => "pptx",
        }
    }
}

#[derive(Debug)]
pub struct Attachment {
    /// The file name the object carried, or empty.
    pub name: String,
    pub kind: Kind,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub found: Vec<Attachment>,
    /// Objects that are in the document but cannot be read (old binary formats, linked files, damaged data).
    pub unreadable: usize,
}

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')?;
    Some(tag[start..start + end].to_owned())
}

/// `(relationship id)` of every `<o:OLEObject>` in the order they appear in the text.
fn object_ids(document_xml: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut from = 0;
    while let Some(at) = document_xml[from..].find("OLEObject") {
        let pos = from + at;
        from = pos + "OLEObject".len();
        let before = &document_xml[..pos];
        if !(before.ends_with("<o:") || before.ends_with('<')) {
            continue;
        }
        let Some(end) = document_xml[pos..].find('>') else { break };
        let tag = &document_xml[pos..pos + end];
        if let Some(id) = attr(tag, "r:id") {
            ids.push(id);
        }
    }
    ids
}

/// `(id, target)` of every relationship in a `.rels` file.
fn relationships(rels_xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for chunk in rels_xml.split("<Relationship ").skip(1) {
        let tag = chunk.split('>').next().unwrap_or("");
        let tag = format!(" {tag}");
        if let (Some(id), Some(target)) = (attr(&tag, "Id"), attr(&tag, "Target")) {
            out.push((id, target));
        }
    }
    out
}

fn cstr(bytes: &[u8]) -> (&[u8], &[u8]) {
    match bytes.iter().position(|b| *b == 0) {
        Some(i) => (&bytes[..i], &bytes[i + 1..]),
        None => (bytes, &[]),
    }
}

/// Works out what a stored object is, looking inside an OLE container if it is one.
fn classify(bytes: &[u8]) -> Option<Attachment> {
    if bytes.starts_with(b"%PDF-") {
        return Some(Attachment { name: String::new(), kind: Kind::Pdf, bytes: bytes.to_vec() });
    }
    if bytes.starts_with(b"PK\x03\x04") {
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
        let has = |n: &str| zip.file_names().any(|f| f.is_ok_and(|name| name == n));
        let kind = if has("word/document.xml") {
            Kind::Word
        } else if has("xl/workbook.xml") {
            Kind::Excel
        } else if has("ppt/presentation.xml") {
            Kind::Slides
        } else {
            return None;
        };
        return Some(Attachment { name: String::new(), kind, bytes: bytes.to_vec() });
    }
    if bytes.starts_with(&CFB_MAGIC) {
        let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
        for stream in ["/CONTENTS", "/Package"] {
            if let Ok(mut s) = cf.open_stream(stream) {
                let mut data = Vec::new();
                if s.read_to_end(&mut data).is_ok()
                    && let Some(found) = classify(&data)
                {
                    return Some(found);
                }
            }
        }
        if let Ok(mut s) = cf.open_stream("/\u{1}Ole10Native") {
            let mut data = Vec::new();
            s.read_to_end(&mut data).ok()?;
            // size (4), type (2), label, source path; the file's own bytes come after a few more header fields
            let head = &data[..data.len().min(4096)];
            let at = (0..head.len()).find(|i| head[*i..].starts_with(b"%PDF-") || head[*i..].starts_with(b"PK\x03\x04"))?;
            let mut found = classify(&data[at..])?;
            if data.len() > 6 {
                let (_label, rest) = cstr(&data[6..]);
                let (file, _) = cstr(rest);
                if let Ok(name) = std::str::from_utf8(file) {
                    found.name = name.rsplit(['\\', '/']).next().unwrap_or("").to_owned();
                }
            }
            return Some(found);
        }
    }
    None
}

/// Reads the embedded objects of a `.docx` in the order they appear. A file that is not a Word package gives an
/// empty result, so this is safe to call on anything.
pub fn scan(docx: &Path) -> Scan {
    let mut out = Scan::default();
    let Ok(file) = std::fs::File::open(docx) else { return out };
    let Ok(mut zip) = zip::ZipArchive::new(file) else { return out };
    let read = |zip: &mut zip::ZipArchive<std::fs::File>, name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    };
    let Some(document) = read(&mut zip, "word/document.xml") else { return out };
    let ids = object_ids(&String::from_utf8_lossy(&document));
    if ids.is_empty() {
        return out;
    }
    let rels = read(&mut zip, "word/_rels/document.xml.rels").map(|b| relationships(&String::from_utf8_lossy(&b))).unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    for id in ids {
        let Some((_, target)) = rels.iter().find(|(i, _)| *i == id) else {
            out.unreadable += 1;
            continue;
        };
        let target = target.trim_start_matches('/');
        let path = if target.starts_with("word/") { target.to_owned() } else { format!("word/{target}") };
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        match read(&mut zip, &path).and_then(|b| classify(&b)) {
            Some(att) => out.found.push(att),
            None => out.unreadable += 1,
        }
    }
    out
}

/// English text when the language catalog has no entry for `key` (the hub and the command line have none).
fn say(key: &str, english: &str, args: &[(&str, &str)]) -> String {
    let text = tf(key, args);
    if text != key {
        return text;
    }
    let mut out = english.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

fn to_pdf(att: &Attachment, tmp: &Path, n: usize, word_cli: &Path) -> Result<PathBuf, String> {
    let dir = tmp.join(format!("att{n}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let input = dir.join(format!("in.{}", att.kind.ext()));
    std::fs::write(&input, &att.bytes).map_err(|e| e.to_string())?;
    if att.kind == Kind::Pdf {
        return Ok(input);
    }
    let cli = match att.kind {
        Kind::Word => Some(word_cli.to_path_buf()),
        Kind::Excel => GRIDCRAFT.find(),
        _ => DECKCRAFT.find(),
    }
    .ok_or_else(|| "the engine for this kind of document was not found".to_owned())?;
    let out = dir.join("out.pdf");
    let mut cmd = std::process::Command::new(cli);
    cmd.arg("convert").arg(&input).arg(&out);
    run_command(&mut cmd, &dir)?;
    if out.is_file() { Ok(out) } else { Err("the engine wrote no file".to_owned()) }
}

/// Adds the pages of the documents embedded in `docx` to the end of `pdf` (the conversion of `docx`), one
/// bookmark each. Never fails the conversion: what cannot be added is reported through `note`.
pub fn append_to_pdf(docx: &Path, pdf: &Path, word_cli: &Path, note: &dyn Fn(String)) {
    let scan = scan(docx);
    if scan.found.is_empty() {
        if scan.unreadable > 0 {
            note(say("embed.skipped", "Embedded objects that could not be read and stay as a picture: {n}", &[("n", &scan.unreadable.to_string())]));
        }
        return;
    }
    let Ok(tmp) = scratch_dir() else { return };
    let mut unreadable = scan.unreadable;
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    for (i, att) in scan.found.iter().enumerate() {
        match to_pdf(att, &tmp, i, word_cli) {
            Ok(p) => {
                let title = if att.name.is_empty() { format!("Attachment {}", parts.len() + 1) } else { att.name.clone() };
                parts.push((title, p));
            }
            Err(_) => unreadable += 1,
        }
    }
    if !parts.is_empty() {
        let main_title = docx.file_stem().and_then(|s| s.to_str()).unwrap_or("Document").to_owned();
        let merged = tmp.join("merged.pdf");
        let mut all: Vec<(String, &Path)> = vec![(main_title, pdf)];
        all.extend(parts.iter().map(|(t, p)| (t.clone(), p.as_path())));
        let done = merge_with_bookmarks(&all, &merged).and_then(|()| std::fs::copy(&merged, pdf).map(|_| ()).map_err(|e| e.to_string()));
        match done {
            Ok(()) => note(say("embed.added", "Embedded documents added at the end of the PDF: {n}", &[("n", &parts.len().to_string())])),
            Err(why) => note(say("embed.failed", "The embedded documents could not be added: {why}", &[("why", &why)])),
        }
    }
    if unreadable > 0 {
        note(say("embed.skipped", "Embedded objects that could not be read and stay as a picture: {n}", &[("n", &unreadable.to_string())]));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Whether `Spec` is a conversion of a Word package to PDF, the one case where embedded documents matter.
pub fn applies(spec: &Spec, src: &Path) -> bool {
    let ext = src.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    spec.out_ext == "pdf" && matches!(ext.as_str(), "docx" | "docm" | "dotx" | "dotm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_object_ids_and_relationships() {
        let xml = r#"<w:r><w:object><v:shape/><o:OLEObject Type="Embed" ProgID="Acrobat.Document.DC" r:id="rId7"/></w:object></w:r><o:OLEObject ProgID="X" r:id="rId9" />"#;
        assert_eq!(object_ids(xml), vec!["rId7", "rId9"]);
        let rels = r#"<Relationships><Relationship Id="rId7" Type="t" Target="embeddings/oleObject1.bin"/><Relationship Target="media/a.png" Id="rId8"/></Relationships>"#;
        let r = relationships(rels);
        assert_eq!(r[0], ("rId7".to_owned(), "embeddings/oleObject1.bin".to_owned()));
        assert_eq!(r[1], ("rId8".to_owned(), "media/a.png".to_owned()));
    }

    #[test]
    fn plain_text_is_not_an_attachment() {
        assert!(classify(b"just text").is_none());
        assert_eq!(classify(b"%PDF-1.7 x").unwrap().kind, Kind::Pdf);
    }

    /// A `.docx` whose text is `text` and which holds one Acrobat-style OLE object (`pdf` in `CONTENTS`) and one
    /// old-style object that is not readable.
    fn docx_with_objects(path: &Path, text: &str, pdf: &[u8]) {
        use std::io::Write;
        let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        ole.create_stream("/CONTENTS").unwrap().write_all(pdf).unwrap();
        let acrobat = ole.into_inner().into_inner();
        let mut old = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        old.create_stream("/WordDocument").unwrap().write_all(b"binary doc").unwrap();
        let old = old.into_inner().into_inner();

        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opt = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &[u8]| {
            zip.start_file(name, opt).unwrap();
            zip.write_all(data).unwrap();
        };
        put("[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="bin" ContentType="application/vnd.openxmlformats-officedocument.oleObject"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#);
        put("_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#);
        put("word/_rels/document.xml.rels", br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="embeddings/oleObject1.bin"/><Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="embeddings/oleObject2.bin"/></Relationships>"#);
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:o="urn:schemas-microsoft-com:office:office"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p><w:p><w:r><w:object><o:OLEObject Type="Embed" ProgID="Acrobat.Document.DC" r:id="rId5"/></w:object></w:r></w:p><w:p><w:r><w:object><o:OLEObject Type="Embed" ProgID="Word.Document.8" r:id="rId6"/></w:object></w:r></w:p></w:body></w:document>"#
        );
        put("word/document.xml", body.as_bytes());
        put("word/embeddings/oleObject1.bin", &acrobat);
        put("word/embeddings/oleObject2.bin", &old);
        zip.finish().unwrap();
    }

    #[test]
    fn scan_reads_a_pdf_in_an_ole_container_and_counts_the_unreadable() {
        let dir = std::env::temp_dir().join("vf_embed_scan");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let docx = dir.join("a.docx");
        docx_with_objects(&docx, "Outer", b"%PDF-1.7 fake");
        let s = scan(&docx);
        assert_eq!(s.found.len(), 1);
        assert_eq!(s.found[0].kind, Kind::Pdf);
        assert_eq!(s.found[0].bytes, b"%PDF-1.7 fake");
        assert_eq!(s.unreadable, 1, "the old binary Word object is counted, not guessed at");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With the real WordCraft (skipped without it): the embedded PDF's pages follow the converted document.
    #[test]
    fn real_engine_appends_the_embedded_pdf() {
        let Some(cli) = crate::engine::WORDCRAFT.find() else { return };
        let dir = std::env::temp_dir().join("vf_embed_real");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rtf = dir.join("inner.rtf");
        std::fs::write(&rtf, br"{\rtf1\ansi Inner document page.\par}").unwrap();
        let spec = Spec { cli, out_ext: "pdf".into(), extra_args: vec![], env: vec![] };
        let inner_pdf = crate::job::convert_one(&spec, &rtf, &dir).expect("inner pdf");
        let docx = dir.join("outer.docx");
        docx_with_objects(&docx, "Outer", &std::fs::read(&inner_pdf).unwrap());
        let notes = std::sync::Mutex::new(Vec::new());
        let out = crate::job::convert_one_noted(&spec, &docx, &dir, &|n| notes.lock().unwrap().push(n)).expect("outer pdf");
        let pages = |p: &Path| lopdf::Document::load(p).unwrap().get_pages().len();
        assert_eq!(pages(&out), pages(&inner_pdf) + 1, "outer page plus the inner document");
        let notes = notes.into_inner().unwrap();
        assert!(notes.iter().any(|n| n.contains("added")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("could not be read")), "{notes:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_package_scans_empty() {
        let p = std::env::temp_dir().join("vf_embed_not_zip.docx");
        std::fs::write(&p, b"nope").unwrap();
        let s = scan(&p);
        assert!(s.found.is_empty() && s.unreadable == 0);
        let _ = std::fs::remove_file(p);
    }
}
