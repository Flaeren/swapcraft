//! E-books in Rust: read an EPUB, build one, and turn a book into plain text or one self-contained HTML page.
//!
//! A [`Book`] is the common shape every reader fills in (EPUB here, Kindle files in `mobi.rs`, text and HTML
//! below) and every writer takes. Chapters hold well-formed XHTML (see `markup.rs`), images are collected into
//! the book and renamed, so nothing depends on where the source files were.
//!
//! Protected books are refused, not worked around: an EPUB with Adobe or other DRM says so and stops.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::markup::{self, Tok, attr, tokenize};

/// An e-book that is protected cannot be converted, and nothing here tries to remove the protection.
pub const DRM_MESSAGE: &str = "This book is protected by DRM (copy protection). voidflare only converts books without it.";

#[derive(Clone, Debug, Default)]
pub struct Book {
    pub title: String,
    pub authors: Vec<String>,
    /// A BCP 47 tag, `en` when the book does not say.
    pub language: String,
    pub chapters: Vec<Chapter>,
    pub images: Vec<Image>,
    /// The book's own style sheets, made safe to put in a page (see [`safe_css`]).
    pub css: String,
}

#[derive(Clone, Debug)]
pub struct Chapter {
    pub title: String,
    /// Well-formed XHTML for the inside of `<body>`.
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct Image {
    /// Where the chapters point at it: `images/img001.jpg`.
    pub name: String,
    pub mime: &'static str,
    pub data: Vec<u8>,
}

/// Collects the images of a book being built: gives each a plain name and never stores one twice.
#[derive(Default)]
pub struct Images {
    pub list: Vec<Image>,
    by_key: HashMap<String, String>,
}

pub fn sniff_mime(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(("image/png", "png"))
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if data.starts_with(b"GIF8") {
        Some(("image/gif", "gif"))
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else if data.starts_with(b"<svg") || data.starts_with(b"<?xml") && String::from_utf8_lossy(&data[..data.len().min(300)]).contains("<svg") {
        Some(("image/svg+xml", "svg"))
    } else {
        None
    }
}

impl Images {
    /// The book's name for these bytes, or `None` when they are not a picture we know.
    pub fn add(&mut self, key: &str, data: Vec<u8>) -> Option<String> {
        if let Some(name) = self.by_key.get(key) {
            return Some(name.clone());
        }
        let (mime, ext) = sniff_mime(&data)?;
        let name = format!("images/img{:03}.{ext}", self.list.len() + 1);
        self.list.push(Image { name: name.clone(), mime, data });
        self.by_key.insert(key.to_owned(), name.clone());
        Some(name)
    }
}

// ---------------------------------------------------------------- base64 and dates

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `2026-11-02T10:30:00Z` for now.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn uuid_for(title: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let h = Sha256::digest(format!("{title}{nanos}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

// ---------------------------------------------------------------- reading an EPUB

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len() + 0
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `dir` + `href` with `..` resolved, the fragment and query removed, `%20` decoded. `None` for a web address.
pub fn resolve(dir: &str, href: &str) -> Option<String> {
    if href.contains("://") || href.starts_with("data:") || href.starts_with("mailto:") {
        return None;
    }
    let path = href.split(['#', '?']).next().unwrap_or("");
    let joined = if path.starts_with('/') { path.trim_start_matches('/').to_owned() } else { format!("{dir}{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for p in joined.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Some(percent_decode(&parts.join("/")))
}

fn text_of(toks: &[Tok], from: usize) -> String {
    let mut s = String::new();
    for t in &toks[from..] {
        match t {
            Tok::Text(x) => s.push_str(x),
            Tok::End(_) => break,
            _ => {}
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title of a chapter: its first heading, else its `<title>`.
fn chapter_title(toks: &[Tok]) -> Option<String> {
    for name in [["h1", "h2", "h3"].as_slice(), ["title"].as_slice()] {
        for (i, t) in toks.iter().enumerate() {
            if let Tok::Start { name: n, .. } = t
                && name.contains(&n.as_str())
            {
                // a heading can hold tags (`<h2><span>I</span> The Machine</h2>`): gather all its text
                let mut depth = 0usize;
                let mut s = String::new();
                for t in &toks[i..] {
                    match t {
                        Tok::Start { .. } => depth += 1,
                        Tok::End(_) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Tok::Text(x) => s.push_str(x),
                    }
                }
                let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

/// Reads an EPUB (2 or 3) from its bytes.
pub fn read_epub(bytes: &[u8]) -> Result<Book, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not an EPUB file ({e})"))?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        f.by_ref().take(400 << 20).read_to_end(&mut v).ok()?;
        Some(v)
    };
    // Protected books: Adobe/other rights files, or encryption that is more than font obfuscation.
    if read("META-INF/rights.xml").is_some() {
        return Err(DRM_MESSAGE.to_owned());
    }
    if let Some(enc) = read("META-INF/encryption.xml") {
        let text = String::from_utf8_lossy(&enc);
        let algorithms: Vec<&str> = text.split("Algorithm=\"").skip(1).filter_map(|p| p.split('"').next()).collect();
        if algorithms.iter().any(|a| !a.contains("idpf.org/2008/embedding") && !a.contains("ns.adobe.com/pdf/enc#RC")) {
            return Err(DRM_MESSAGE.to_owned());
        }
    }
    let container = read("META-INF/container.xml").ok_or("not an EPUB: META-INF/container.xml is missing")?;
    let opf_path = tokenize(&String::from_utf8_lossy(&container))
        .iter()
        .find_map(|t| match t {
            Tok::Start { name, attrs, .. } if name == "rootfile" => attr(attrs, "full-path").map(str::to_owned),
            _ => None,
        })
        .ok_or("not an EPUB: no package document is named")?;
    let opf = String::from_utf8_lossy(&read(&opf_path).ok_or_else(|| format!("the package document {opf_path} is missing"))?).into_owned();
    let opf_dir = opf_path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
    let toks = tokenize(&opf);

    let mut book = Book { language: "en".into(), ..Book::default() };
    let mut manifest: HashMap<String, (String, String)> = HashMap::new();
    let mut spine: Vec<String> = Vec::new();
    // the cover picture: EPUB 3 marks the manifest item, EPUB 2 names its id in the metadata
    let (mut cover_marked, mut cover_named) = (None, None);
    for (i, t) in toks.iter().enumerate() {
        let Tok::Start { name, attrs, .. } = t else { continue };
        // some books write the package elements with a prefix (`opf:item`)
        match name.strip_prefix("opf:").unwrap_or(name) {
            "dc:title" if book.title.is_empty() => book.title = text_of(&toks, i + 1),
            "dc:creator" => {
                let a = text_of(&toks, i + 1);
                if !a.is_empty() {
                    book.authors.push(a);
                }
            }
            "dc:language" => {
                let l = text_of(&toks, i + 1);
                if !l.is_empty() {
                    book.language = l;
                }
            }
            "item" => {
                if let (Some(id), Some(href)) = (attr(attrs, "id"), attr(attrs, "href")) {
                    manifest.insert(id.to_owned(), (href.to_owned(), attr(attrs, "media-type").unwrap_or("").to_owned()));
                    if attr(attrs, "properties").is_some_and(|p| p.split_whitespace().any(|w| w == "cover-image")) {
                        cover_marked = Some(id.to_owned());
                    }
                }
            }
            "meta" if attr(attrs, "name") == Some("cover") => cover_named = attr(attrs, "content").map(str::to_owned),
            "itemref" => {
                if let Some(id) = attr(attrs, "idref") {
                    spine.push(id.to_owned());
                }
            }
            _ => {}
        }
    }
    if spine.is_empty() {
        return Err("this EPUB has no reading order (spine)".to_owned());
    }
    let mut images = Images::default();
    // the pages in reading order, read first: a link can point at a page further on
    let mut pages: Vec<(String, Vec<Tok>)> = Vec::new();
    // the style the pages ask for, in the order they ask: a package can carry themes no page uses
    let mut sheets: Vec<String> = Vec::new();
    let mut linked_sheets: Vec<String> = Vec::new();
    for id in spine {
        let Some((href, media)) = manifest.get(&id) else { continue };
        if !(media.contains("html") || media.is_empty()) {
            continue;
        }
        let Some(path) = resolve(&opf_dir, href) else { continue };
        let Some(raw) = read(&path) else { continue };
        let text = String::from_utf8_lossy(&raw);
        let toks = tokenize(&text);
        let dir = path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
        for t in &toks {
            let Tok::Start { name, attrs, .. } = t else { continue };
            if name == "link"
                && attr(attrs, "rel").is_some_and(|r| r.eq_ignore_ascii_case("stylesheet"))
                && !attr(attrs, "media").is_some_and(|m| m.contains("amzn"))
                && let Some(sheet) = attr(attrs, "href").and_then(|h| resolve(&dir, h))
                && !linked_sheets.contains(&sheet)
            {
                if let Some(data) = read(&sheet) {
                    sheets.push(String::from_utf8_lossy(&data).into_owned());
                }
                linked_sheets.push(sheet);
            }
        }
        for css in inline_styles(&text) {
            if !sheets.iter().any(|s| s == css) {
                sheets.push(css.to_owned());
            }
        }
        pages.push((path, toks));
    }
    let links = Links::of(&pages);
    for (path, toks) in pages {
        let dir = path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
        let linked = markup::Linked {
            href: &|href| links.href(&path, href),
            anchor: &|id| links.anchor(&path, id),
        };
        let mut body = markup::to_linked_xhtml(
            &toks,
            &mut |src| {
                let p = resolve(&dir, src)?;
                let data = read(&p)?;
                images.add(&p, data)
            },
            &linked,
        );
        if body.trim().is_empty() {
            continue;
        }
        if let Some(key) = links.anchor(&path, "") {
            body.insert_str(0, &format!("<a name=\"{key}\"></a>"));
        }
        let title = chapter_title(&toks).unwrap_or_else(|| format!("Chapter {}", book.chapters.len() + 1));
        book.chapters.push(Chapter { title, body });
    }
    book.css = safe_css(&sheets.join("\n"));
    if book.title.is_empty() {
        book.title = "Untitled".into();
    }
    if book.chapters.is_empty() {
        return Err("this EPUB has no readable text".to_owned());
    }
    if let Some((href, media)) = cover_marked.or(cover_named).and_then(|id| manifest.get(&id))
        && media.starts_with("image/")
        && let Some(path) = resolve(&opf_dir, href)
        && let Some(data) = read(&path)
        && let Some(name) = images.add(&path, data)
    {
        put_cover_first(&mut book.chapters, &name);
    }
    book.images = images.list;
    Ok(book)
}

/// The places the book's links lead to, each under a short name: (page, id), the id empty for the page itself.
struct Links {
    keys: HashMap<(String, String), String>,
}

/// The page and id a link of the page at `from` leads to, `None` for an address outside the book.
fn link_target(from: &str, href: &str) -> Option<(String, String)> {
    if href.split(['/', '?', '#']).next().is_some_and(|first| first.contains(':')) {
        return None;
    }
    let (file, id) = href.split_once('#').unwrap_or((href, ""));
    let page = if file.is_empty() {
        from.to_owned()
    } else {
        let dir = from.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
        resolve(&dir, file)?
    };
    Some((page, percent_decode(id)))
}

impl Links {
    fn of(pages: &[(String, Vec<Tok>)]) -> Links {
        // what exists: every page, and every id in it
        let mut there: Vec<(&str, &str)> = Vec::new();
        for (path, toks) in pages {
            there.push((path, ""));
            there.extend(toks.iter().filter_map(|t| match t {
                Tok::Start { attrs, .. } => attr(attrs, "id").map(|id| (path.as_str(), id)),
                _ => None,
            }));
        }
        let mut keys = HashMap::new();
        for (path, toks) in pages {
            for t in toks {
                let Tok::Start { name, attrs, .. } = t else { continue };
                if name != "a" {
                    continue;
                }
                let Some((page, id)) = attr(attrs, "href").and_then(|h| link_target(path, h)) else { continue };
                // an id that is not there still opens the right page
                let id = if there.contains(&(page.as_str(), id.as_str())) { id } else { String::new() };
                if there.contains(&(page.as_str(), "")) {
                    let n = keys.len() + 1;
                    keys.entry((page, id)).or_insert_with(|| format!("swc{n}"));
                }
            }
        }
        Links { keys }
    }

    fn anchor(&self, page: &str, id: &str) -> Option<String> {
        self.keys.get(&(page.to_owned(), id.to_owned())).cloned()
    }

    /// Web and mail addresses stay; a link inside the book goes to its anchor; anything else is dropped.
    fn href(&self, from: &str, href: &str) -> Option<String> {
        match link_target(from, href) {
            None => ["http:", "https:", "mailto:"].iter().any(|s| href.starts_with(s)).then(|| href.to_owned()),
            Some((page, id)) => self.anchor(&page, &id).or_else(|| self.anchor(&page, "")).map(|key| format!("#{key}")),
        }
    }
}

/// What is written between `<style>` and `</style>` (the tokenizer keeps only the tags).
fn inline_styles(page: &str) -> Vec<&str> {
    // ASCII lowering keeps byte positions
    let lower = page.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = lower[from..].find("<style").map(|i| from + i) {
        let Some(start) = lower[open..].find('>').map(|i| open + i + 1) else { break };
        let Some(end) = lower[start..].find("</style").map(|i| start + i) else { break };
        out.push(&page[start..end]);
        from = end;
    }
    out
}

/// A book's style sheet made safe for our page: nothing it names is fetched, and it cannot close its own tag.
fn safe_css(css: &str) -> String {
    let mut out = String::new();
    // no backslashes: an escape could spell `url(` in disguise
    for line in css.replace(['<', '\\'], "").lines() {
        if line.trim_start().to_ascii_lowercase().starts_with("@import") {
            continue;
        }
        let mut rest = line;
        // ASCII lowering keeps byte positions
        while let Some(at) = rest.to_ascii_lowercase().find("url(") {
            out.push_str(&rest[..at]);
            out.push_str("none(");
            rest = &rest[at + 4..];
        }
        out.push_str(rest);
        out.push('\n');
    }
    out
}

/// A page that is a picture and nothing else.
fn is_picture_only(body: &str) -> bool {
    body.contains("<img") && markup::to_text(&tokenize(body)).trim().is_empty()
}

/// Makes the book open with its cover: the cover page of the first few is moved to the front, a book without one
/// gets a page with the picture. A picture that shares its page with text stays where the author put it.
fn put_cover_first(chapters: &mut Vec<Chapter>, image: &str) {
    let source = format!("src=\"{image}\"");
    match chapters.iter().take(3).position(|c| c.body.contains(&source)) {
        Some(i) if is_picture_only(&chapters[i].body) => {
            let cover = chapters.remove(i);
            chapters.insert(0, cover);
        }
        Some(_) => {}
        None => chapters.insert(0, Chapter { title: "Cover".into(), body: format!("<img {source} alt=\"\"/>") }),
    }
}

// ---------------------------------------------------------------- other sources

/// A book from plain text: blank lines separate paragraphs, line breaks inside one are joined.
/// Plain text as a document: paragraphs at blank lines, and every line break kept (a list, an address, a poem
/// stay as written). [`from_text`] joins the lines instead, for e-books whose text was wrapped by hand.
pub fn from_text_lines(text: &str, title: &str) -> Book {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let body: String = text
        .split("\n\n")
        .map(|p| p.trim_matches('\n'))
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", p.lines().map(|l| markup::escape_text(l.trim_end())).collect::<Vec<_>>().join("<br/>")))
        .collect();
    Book { title: title.to_owned(), language: "en".into(), chapters: vec![Chapter { title: title.to_owned(), body }], ..Book::default() }
}

pub fn from_text(text: &str, title: &str) -> Book {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let body: String = text
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", markup::escape_text(&p)))
        .collect();
    Book { title: title.to_owned(), language: "en".into(), chapters: vec![Chapter { title: title.to_owned(), body }], ..Book::default() }
}

/// A book from an HTML page. Images given as `data:` addresses or as files next to the page are packed into
/// the book; web addresses are never fetched. Each `<h1>` starts a chapter.
pub fn from_html(html: &str, base: Option<&Path>, fallback_title: &str) -> Book {
    let toks = tokenize(html);
    let title = toks
        .iter()
        .position(|t| matches!(t, Tok::Start { name, .. } if name == "title"))
        .map(|i| text_of(&toks, i + 1))
        .filter(|t| !t.is_empty())
        .or_else(|| chapter_title(&toks))
        .unwrap_or_else(|| fallback_title.to_owned());
    // Chapters start at each <h1>, unless the page uses <h2> for them (a word processor's "Heading 2" under a
    // single title): then the <h2> are the chapters.
    let heading_count = |h: &str| toks.iter().filter(|t| matches!(t, Tok::Start { name, .. } if name == h)).count();
    let (h1, h2) = (heading_count("h1"), heading_count("h2"));
    let chapter_tag = if h2 >= 3 && h2 > h1 * 2 { "h2" } else { "h1" };
    let mut cuts: Vec<usize> = toks.iter().enumerate().filter(|(_, t)| matches!(t, Tok::Start { name, .. } if name == chapter_tag)).map(|(i, _)| i).collect();
    // only the part of the page inside <body> counts for the first chapter
    let body_start = toks.iter().position(|t| matches!(t, Tok::Start { name, .. } if name == "body")).unwrap_or(0);
    if cuts.first().is_none_or(|&c| c > body_start + 1) {
        cuts.insert(0, body_start);
    }
    cuts.push(toks.len());
    let mut images = Images::default();
    let mut chapters = Vec::new();
    for w in cuts.windows(2) {
        let part = &toks[w[0]..w[1]];
        let body = markup::to_xhtml(part, &mut |src| {
            if let Some(rest) = src.strip_prefix("data:") {
                let (_, b64) = rest.split_once(";base64,")?;
                return images.add(&format!("data:{}", &b64[..b64.len().min(64)]), base64_decode(b64)?);
            }
            let p = base?.join(resolve("", src)?);
            images.add(&p.to_string_lossy(), std::fs::read(&p).ok()?)
        });
        if body.trim().is_empty() {
            continue;
        }
        let title = chapter_title(part).unwrap_or_else(|| format!("Chapter {}", chapters.len() + 1));
        chapters.push(Chapter { title, body });
    }
    if chapters.is_empty() {
        chapters.push(Chapter { title: title.clone(), body: "<p></p>".into() });
    }
    Book { title, language: "en".into(), chapters, images: images.list, ..Book::default() }
}

// ---------------------------------------------------------------- writing

impl Book {
    /// Plain text: the title, the authors, then the chapters.
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", self.title);
        if !self.authors.is_empty() {
            out.push_str(&format!("{}\n", self.authors.join(", ")));
        }
        out.push('\n');
        for c in &self.chapters {
            out.push_str(&markup::to_text(&tokenize(&c.body)));
            out.push('\n');
        }
        out.trim_end().to_owned() + "\n"
    }

    /// One HTML page with the images inside it, so it opens anywhere and can be handed to a converter.
    pub fn to_html(&self) -> String {
        self.html(true)
    }

    /// The page as a document, not a book: no title page in front (a note or a web page made into a PDF starts
    /// with its own first line, not with its file name alone on page one).
    pub fn to_document_html(&self) -> String {
        self.html(false)
    }

    fn html(&self, title_page: bool) -> String {
        let mut body = String::new();
        // a book that opens with its own title page does not need the title said twice
        let opening: String = self.chapters.first().map(|c| markup::to_text(&tokenize(&c.body))).unwrap_or_default().chars().take(200).collect();
        // nor does one that opens with its cover
        let covered = self.chapters.first().is_some_and(|c| is_picture_only(&c.body));
        if title_page && !covered && !opening.to_lowercase().contains(&self.title.to_lowercase()) {
            body.push_str(&format!("<h1 class=\"book-title\">{}</h1>", markup::escape_text(&self.title)));
            if !self.authors.is_empty() {
                body.push_str(&format!("<p class=\"book-author\">{}</p>", markup::escape_text(&self.authors.join(", "))));
            }
        }
        for c in &self.chapters {
            // our own class: a rule on `section` would also break the page at every section inside the book
            body.push_str("<div class=\"swc-page\">");
            if is_picture_only(&c.body) {
                body.push_str(&format!("<div style=\"text-align:center\">{}</div>", c.body));
            } else {
                body.push_str(&c.body);
            }
            body.push_str("</div>");
        }
        for img in &self.images {
            body = body.replace(&format!("src=\"{}\"", img.name), &format!("src=\"data:{};base64,{}\"", img.mime, base64_encode(&img.data)));
        }
        format!(
            "<!DOCTYPE html>\n<html lang=\"{lang}\"><head><meta charset=\"utf-8\"/><title>{title}</title><style>{css}\n{own}{last}</style></head><body>{body}</body></html>\n",
            lang = markup::escape_attr(&self.language),
            title = markup::escape_text(&self.title),
            css = CSS,
            own = self.css,
            // what must hold whatever the book's sheet says
            last = ".swc-page{page-break-before:always}img{max-width:100%;height:auto}",
        )
    }

    /// An EPUB 3 file (with the older NCX contents too, for readers that want it).
    pub fn to_epub(&self) -> Result<Vec<u8>, String> {
        let e = |x: zip::result::ZipError| x.to_string();
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // the first entry must be "mimetype", stored, with no extra fields: that is how readers recognise an EPUB
        z.start_file("mimetype", stored).map_err(e)?;
        z.write_all(b"application/epub+zip").map_err(|x| x.to_string())?;
        let mut put = |name: &str, data: &[u8]| -> Result<(), String> {
            z.start_file(name, deflated).map_err(e)?;
            z.write_all(data).map_err(|x| x.to_string())
        };
        put(
            "META-INF/container.xml",
            b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>\n",
        )?;
        put("OEBPS/style.css", format!("{CSS}\n{}", self.css).as_bytes())?;
        let lang = markup::escape_attr(&self.language);
        let mut manifest = String::from(
            "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/><item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>",
        );
        let (mut spine, mut nav, mut ncx) = (String::new(), String::new(), String::new());
        for (i, c) in self.chapters.iter().enumerate() {
            let file = format!("chapter-{:03}.xhtml", i + 1);
            let title = markup::escape_text(&c.title);
            let page = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{lang}\" xml:lang=\"{lang}\"><head><meta charset=\"utf-8\"/><title>{title}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head><body>{}</body></html>\n",
                c.body
            );
            put(&format!("OEBPS/{file}"), page.as_bytes())?;
            manifest.push_str(&format!("<item id=\"c{n}\" href=\"{file}\" media-type=\"application/xhtml+xml\"/>", n = i + 1));
            spine.push_str(&format!("<itemref idref=\"c{}\"/>", i + 1));
            nav.push_str(&format!("<li><a href=\"{file}\">{title}</a></li>"));
            ncx.push_str(&format!("<navPoint id=\"n{n}\" playOrder=\"{n}\"><navLabel><text>{title}</text></navLabel><content src=\"{file}\"/></navPoint>", n = i + 1));
        }
        for (i, img) in self.images.iter().enumerate() {
            put(&format!("OEBPS/{}", img.name), &img.data)?;
            manifest.push_str(&format!("<item id=\"i{}\" href=\"{}\" media-type=\"{}\"/>", i + 1, img.name, img.mime));
        }
        let title = markup::escape_text(&self.title);
        let id = uuid_for(&self.title);
        put(
            "OEBPS/nav.xhtml",
            format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{lang}\" xml:lang=\"{lang}\"><head><meta charset=\"utf-8\"/><title>{title}</title></head><body><nav epub:type=\"toc\" id=\"toc\"><h1>{title}</h1><ol>{nav}</ol></nav></body></html>\n").as_bytes(),
        )?;
        put(
            "OEBPS/toc.ncx",
            format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><head><meta name=\"dtb:uid\" content=\"urn:uuid:{id}\"/></head><docTitle><text>{title}</text></docTitle><navMap>{ncx}</navMap></ncx>\n").as_bytes(),
        )?;
        let creators: String = self.authors.iter().map(|a| format!("<dc:creator>{}</dc:creator>", markup::escape_text(a))).collect();
        put(
            "OEBPS/content.opf",
            format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"{lang}\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"bookid\">urn:uuid:{id}</dc:identifier><dc:title>{title}</dc:title><dc:language>{lang}</dc:language>{creators}<meta property=\"dcterms:modified\">{}</meta></metadata><manifest>{manifest}</manifest><spine toc=\"ncx\">{spine}</spine></package>\n", utc_now()).as_bytes(),
        )?;
        Ok(z.finish().map_err(e)?.into_inner())
    }
}

const CSS: &str = "body{font-family:Georgia,'Times New Roman',serif;line-height:1.5;margin:1em auto;max-width:42em;padding:0 1em}\
h1,h2,h3{line-height:1.25}p{margin:0 0 .8em}img{max-width:100%;height:auto}blockquote{margin:1em 2em}\
.book-title{text-align:center;margin-top:2em}.book-author{text-align:center;font-style:italic;margin-bottom:3em}\
.swc-page{page-break-before:always}table{border-collapse:collapse}td,th{border:1px solid #888;padding:.2em .5em}";

#[cfg(test)]
mod tests {
    use super::*;

    /// A small EPUB: `pages` are the files of the reading order, `extra` goes into the manifest as it is.
    fn epub_with(pages: &[(&str, &str)], extra: &str, metadata: &str) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &[u8]| {
            z.start_file(name, options).unwrap();
            z.write_all(data).unwrap();
        };
        put("META-INF/container.xml", b"<container><rootfiles><rootfile full-path=\"p.opf\"/></rootfiles></container>");
        let (mut manifest, mut spine) = (String::new(), String::new());
        for (name, body) in pages {
            manifest.push_str(&format!("<item id=\"{name}\" href=\"{name}.xhtml\" media-type=\"application/xhtml+xml\"/>"));
            spine.push_str(&format!("<itemref idref=\"{name}\"/>"));
            put(&format!("{name}.xhtml"), format!("<html><head><title>{name}</title></head><body>{body}</body></html>").as_bytes());
        }
        put("c.png", b"\x89PNG\r\n\x1a\nnot-really-a-picture");
        put("s.css", b".centre{text-align:center}");
        put("p.opf", format!("<package><metadata><dc:title>Test Book</dc:title>{metadata}</metadata><manifest>{manifest}{extra}</manifest><spine>{spine}</spine></package>").as_bytes());
        z.finish().unwrap().into_inner()
    }

    const COVER_ITEM: &str = "<item id=\"img\" href=\"c.png\" media-type=\"image/png\" properties=\"cover-image\"/>";

    #[test]
    fn hidden_page_numbers_and_landmarks_are_not_printed() {
        let nav = "<nav epub:type=\"toc\"><h1>Contents</h1><ol><li><a href=\"t.xhtml\">One</a><ol><li>Inner</li></ol></li></ol></nav>\
            <nav epub:type=\"page-list\" hidden=\"hidden\"><ol><li>PAGEMARK</li></ol></nav>\
            <nav epub:type=\"landmarks\"><ol><li>LANDMARK</li></ol></nav><nav hidden=\"\"><p>HIDDEN</p></nav><p>After</p>";
        let book = read_epub(&epub_with(&[("nav", nav)], "", "")).unwrap();
        let body = &book.chapters[0].body;
        for gone in ["PAGEMARK", "LANDMARK", "HIDDEN"] {
            assert!(!body.contains(gone), "{gone} is printed: {body}");
        }
        assert!(body.contains("One") && body.contains("Inner") && body.contains("After"), "{body}");
        // the contents are not a numbered list, and the inner level stays indented
        assert!(!body.contains("<ol") && !body.contains("<li"), "{body}");
        assert!(body.contains("margin-left"), "{body}");
    }

    #[test]
    fn a_list_in_the_text_stays_a_list() {
        let book = read_epub(&epub_with(&[("t", "<ol><li>first</li></ol>")], "", "")).unwrap();
        assert!(book.chapters[0].body.contains("<ol><li>first</li></ol>"), "{}", book.chapters[0].body);
    }

    #[test]
    fn only_our_chapters_start_a_new_page() {
        let book = read_epub(&epub_with(&[("t", "<section><p>Title</p></section><section><p>Author</p></section>")], "", "")).unwrap();
        let html = book.to_html();
        assert!(html.contains(".swc-page{page-break-before:always}"), "{html}");
        assert!(!html.contains("section{page-break-before"), "the book's own sections must not break the page");
        assert_eq!(html.matches("<div class=\"swc-page\">").count(), 1, "{html}");
    }

    #[test]
    fn a_cover_drawn_with_svg_opens_the_book_with_no_title_page() {
        let cover = "<svg viewBox=\"0 0 6 9\"><image width=\"6\" height=\"9\" xlink:href=\"c.png\"/></svg>";
        let book = read_epub(&epub_with(&[("cover", cover), ("t", "<p>Text</p>")], COVER_ITEM, "")).unwrap();
        assert_eq!(book.chapters.len(), 2, "the cover is not added twice");
        assert!(book.chapters[0].body.contains("<img src=\"images/"), "{}", book.chapters[0].body);
        assert!(!book.to_html().contains("class=\"book-title\""));
    }

    #[test]
    fn a_cover_named_only_by_the_package_is_put_first() {
        let named_the_old_way = "<item id=\"img\" href=\"c.png\" media-type=\"image/png\"/>";
        for (extra, metadata) in [(COVER_ITEM, ""), (named_the_old_way, "<meta name=\"cover\" content=\"img\"/>")] {
            let book = read_epub(&epub_with(&[("t", "<p>Text</p>")], extra, metadata)).unwrap();
            assert_eq!(book.chapters.len(), 2);
            assert!(book.chapters[0].body.starts_with("<img src=\"images/"), "{}", book.chapters[0].body);
            assert!(!book.to_html().contains("class=\"book-title\""));
        }
    }

    #[test]
    fn a_cover_further_in_moves_to_the_front() {
        let book = read_epub(&epub_with(&[("half", "<p>Half title</p>"), ("cover", "<div><img src=\"c.png\"/></div>")], COVER_ITEM, "")).unwrap();
        assert_eq!(book.chapters.len(), 2);
        assert!(book.chapters[0].body.contains("<img"), "{}", book.chapters[0].body);
    }

    #[test]
    fn links_inside_the_book_point_at_anchors_the_engine_can_follow() {
        let contents = "<p><a href=\"two.xhtml#part\">Part</a> <a href=\"two.xhtml\">Two</a> <a href=\"https://example.org/a#b\">Web</a> <a href=\"gone.xhtml\">Gone</a></p>";
        let two = "<h1 id=\"part\">Part</h1><p id=\"unused\">Text <a href=\"#part\">up</a></p>";
        let book = read_epub(&epub_with(&[("one", contents), ("two", two)], "", "")).unwrap();
        let (one, two) = (&book.chapters[0].body, &book.chapters[1].body);
        let key = |after: &str, body: &str| body.split(after).next().unwrap().rsplit("href=\"#").next().unwrap().split('"').next().unwrap().to_owned();
        let (part, start) = (key(">Part</a>", one), key(">Two</a>", one));
        assert_ne!(part, start);
        assert!(two.contains(&format!("<a name=\"{part}\"></a>")), "{two}");
        assert!(two.starts_with(&format!("<a name=\"{start}\"></a>")), "{two}");
        assert!(two.contains(&format!("href=\"#{part}\">up</a>")), "{two}");
        assert!(one.contains("href=\"https://example.org/a#b\""), "{one}");
        assert!(!one.contains("gone.xhtml"), "a link to nothing is not kept: {one}");
        assert_eq!(two.matches("<a name=").count(), 2, "only places something links to are marked: {two}");
    }

    #[test]
    fn the_books_own_style_comes_along() {
        let page = "<p class=\"centre\" style=\"text-indent:2em\">Title</p>";
        let css = "<item id=\"s\" href=\"s.css\" media-type=\"text/css\"/>";
        let unused = read_epub(&epub_with(&[("t", page)], css, "")).unwrap();
        assert!(unused.css.trim().is_empty(), "a sheet no page asks for is left out: {}", unused.css);
        let page = "<link rel=\"stylesheet\" href=\"s.css\"/><p class=\"centre\" style=\"text-indent:2em\">Title</p>";
        let book = read_epub(&epub_with(&[("t", page)], css, "")).unwrap();
        assert!(book.chapters[0].body.contains("class=\"centre\"") && book.chapters[0].body.contains("style=\"text-indent:2em\""), "{}", book.chapters[0].body);
        assert!(book.to_html().contains(".centre{text-align:center}"), "{}", book.to_html());
    }

    #[test]
    fn a_style_on_an_element_cannot_name_an_address() {
        let page = "<p style=\"background:URL(http://x/p.png)\">a</p><p style=\"background:u\\72l(http://x/q.png)\">b</p><p style=\"text-align:center\">c</p>";
        let book = read_epub(&epub_with(&[("t", page)], "", "")).unwrap();
        let body = &book.chapters[0].body;
        assert!(!body.contains("http://x/") && body.contains("text-align:center"), "{body}");
        assert!(!safe_css("p{background:u\\72l(http://x/y.png)}").contains('\\'));
    }

    #[test]
    fn a_style_sheet_cannot_fetch_anything_or_leave_its_tag() {
        let css = safe_css("@import url(http://x/y.css);\np{color:red;background:URL(http://x/p.png)}</style><script>");
        assert!(!css.to_lowercase().contains("url(") && !css.contains("@import") && !css.contains('<'), "{css}");
        assert!(css.contains("color:red"), "{css}");
    }

    #[test]
    fn a_footnote_hidden_on_screen_is_still_printed() {
        let page = "<style>.note{display:none}</style><p>Text<span class=\"note\">NOTE-ONE</span></p><aside hidden=\"\">NOTE-TWO</aside>";
        let book = read_epub(&epub_with(&[("t", page)], "", "")).unwrap();
        assert!(book.chapters[0].body.contains("NOTE-ONE") && book.chapters[0].body.contains("NOTE-TWO"), "{}", book.chapters[0].body);
    }

    #[test]
    fn a_package_written_with_a_prefix_is_read() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in [
            ("META-INF/container.xml", "<container><rootfiles><rootfile full-path=\"p.opf\"/></rootfiles></container>"),
            ("p.opf", "<opf:package xmlns:opf=\"x\"><opf:manifest><opf:item id=\"t\" href=\"t.xhtml\" media-type=\"application/xhtml+xml\"/></opf:manifest><opf:spine><opf:itemref idref=\"t\"/></opf:spine></opf:package>"),
            ("t.xhtml", "<body><p>Read</p></body>"),
        ] {
            z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        let book = read_epub(&z.finish().unwrap().into_inner()).unwrap();
        assert!(book.chapters[0].body.contains("Read"));
    }

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0, 255, 128, 7, 9]] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn paths_resolve_like_a_browser() {
        assert_eq!(resolve("OEBPS/text/", "../images/a%20b.png#x").as_deref(), Some("OEBPS/images/a b.png"));
        assert_eq!(resolve("OEBPS/", "/abs/x.png").as_deref(), Some("abs/x.png"));
        assert_eq!(resolve("a/", "https://x.org/y.png"), None);
        assert_eq!(resolve("a/", "data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn the_date_is_formatted_like_the_epub_spec_wants() {
        let d = utc_now();
        assert_eq!(d.len(), 20);
        assert!(d.starts_with("20") && d.ends_with('Z') && d.as_bytes()[10] == b'T', "{d}");
        assert_eq!(uuid_for("x").len(), 36);
    }

    #[test]
    fn text_becomes_a_book_that_survives_an_epub_round_trip() {
        let book = from_text("Prima riga\ncontinua qui.\n\nSeconda e caff\u{e8} & t\u{e8} <3.\n\n\n\nTerza.", "Il mio libro");
        assert_eq!(book.chapters[0].body, "<p>Prima riga continua qui.</p><p>Seconda e caff\u{e8} &amp; t\u{e8} &lt;3.</p><p>Terza.</p>");
        let bytes = book.to_epub().unwrap();
        // mimetype first and stored, as readers require
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes[30..30 + 8].starts_with(b"mimetype"));
        let again = read_epub(&bytes).expect("our own EPUB reads back");
        assert_eq!(again.title, "Il mio libro");
        assert_eq!(again.to_text().trim(), book.to_text().trim());
    }

    #[test]
    fn html_with_pictures_and_chapters_makes_an_epub_with_both() {
        let png = [0x89u8, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 0];
        let html = format!(
            "<html><head><title>Raccolta</title></head><body><p>Prologo</p><h1>Uno</h1><p>Testo.<img src=\"data:image/png;base64,{}\"/></p><h1>Due</h1><p>Altro.<img src=\"http://example.com/x.png\"/></p></body></html>",
            base64_encode(&png)
        );
        let book = from_html(&html, None, "x");
        assert_eq!(book.title, "Raccolta");
        assert_eq!(book.chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Chapter 1", "Uno", "Due"]);
        assert_eq!(book.images.len(), 1, "the web picture is never fetched");
        assert!(book.chapters[1].body.contains("images/img001.png"));
        let back = read_epub(&book.to_epub().unwrap()).unwrap();
        assert_eq!(back.chapters.len(), 3);
        assert_eq!(back.images.len(), 1);
        assert!(book.to_html().contains("data:image/png;base64,"));
    }

    #[test]
    fn a_word_processors_heading_2_chapters_become_chapters() {
        let html = "<body><h1>Il libro</h1><p>intro</p><h2>Uno</h2><p>a</p><h2>Due</h2><p>b</p><h2>Tre</h2><p>c</p></body>";
        let book = from_html(html, None, "x");
        let names: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(names, ["Il libro", "Uno", "Due", "Tre"]);
        // pages that use <h1> for chapters keep doing so
        let book = from_html("<body><h1>A</h1><p>x</p><h1>B</h1><p>y</p><h2>sub</h2><h2>sub</h2><h2>sub</h2></body>", None, "x");
        assert_eq!(book.chapters.len(), 2);
    }

    #[test]
    fn a_title_page_is_not_repeated_in_the_html() {
        let mut book = from_text("x", "Moby Dick");
        book.chapters[0].body = "<h1>Moby Dick</h1><p>Call me Ishmael.</p>".into();
        assert_eq!(book.to_html().matches("Moby Dick").count(), 2, "the <title> and the book's own heading, not a third");
        book.chapters[0].body = "<p>Call me Ishmael.</p>".into();
        assert!(book.to_html().contains("class=\"book-title\""));
    }

    #[test]
    fn a_book_with_rights_information_is_refused() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("META-INF/rights.xml", o).unwrap();
        z.write_all(b"<rights/>").unwrap();
        let bytes = z.finish().unwrap().into_inner();
        assert_eq!(read_epub(&bytes).unwrap_err(), DRM_MESSAGE);
    }
}
