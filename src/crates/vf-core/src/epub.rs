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
    for (i, t) in toks.iter().enumerate() {
        let Tok::Start { name, attrs, .. } = t else { continue };
        match name.as_str() {
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
                }
            }
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
    for id in spine {
        let Some((href, media)) = manifest.get(&id) else { continue };
        if !(media.contains("html") || media.is_empty()) {
            continue;
        }
        let Some(path) = resolve(&opf_dir, href) else { continue };
        let Some(raw) = read(&path) else { continue };
        let toks = tokenize(&String::from_utf8_lossy(&raw));
        let dir = path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
        let body = markup::to_xhtml(&toks, &mut |src| {
            let p = resolve(&dir, src)?;
            let data = read(&p)?;
            images.add(&p, data)
        });
        if body.trim().is_empty() {
            continue;
        }
        let title = chapter_title(&toks).unwrap_or_else(|| format!("Chapter {}", book.chapters.len() + 1));
        book.chapters.push(Chapter { title, body });
    }
    book.images = images.list;
    if book.title.is_empty() {
        book.title = "Untitled".into();
    }
    if book.chapters.is_empty() {
        return Err("this EPUB has no readable text".to_owned());
    }
    Ok(book)
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
    Book { title, language: "en".into(), chapters, images: images.list, authors: Vec::new() }
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
        if title_page && !opening.to_lowercase().contains(&self.title.to_lowercase()) {
            body.push_str(&format!("<h1 class=\"book-title\">{}</h1>", markup::escape_text(&self.title)));
            if !self.authors.is_empty() {
                body.push_str(&format!("<p class=\"book-author\">{}</p>", markup::escape_text(&self.authors.join(", "))));
            }
        }
        for c in &self.chapters {
            body.push_str("<section>");
            body.push_str(&c.body);
            body.push_str("</section>");
        }
        for img in &self.images {
            body = body.replace(&format!("src=\"{}\"", img.name), &format!("src=\"data:{};base64,{}\"", img.mime, base64_encode(&img.data)));
        }
        format!(
            "<!DOCTYPE html>\n<html lang=\"{lang}\"><head><meta charset=\"utf-8\"/><title>{title}</title><style>{css}</style></head><body>{body}</body></html>\n",
            lang = markup::escape_attr(&self.language),
            title = markup::escape_text(&self.title),
            css = CSS,
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
        put("OEBPS/style.css", CSS.as_bytes())?;
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
section{page-break-before:always}table{border-collapse:collapse}td,th{border:1px solid #888;padding:.2em .5em}";

#[cfg(test)]
mod tests {
    use super::*;

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
