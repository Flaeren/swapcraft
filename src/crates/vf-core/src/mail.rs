//! Mail conversions: e-mail files (`.eml`), mailboxes (`.mbox`) and Outlook data files (`.pst`) to PDF, to single
//! messages, to a mailbox, or (from a `.pst`) to its calendar and contacts.
//!
//! The messages are read in Rust (`mail-parser`). A message becomes a PDF through WordCraft, from an HTML page
//! made here: the mail's header (From, To, Date, Subject, attachments), then its body cleaned for printing. Pictures
//! stored on the internet are never downloaded (they are counted and named), pictures carried inside the message
//! (`cid:`) are drawn, scripts and hidden blocks go, and the tables newsletters use for layout become plain blocks
//! (WordCraft draws a border round every table). A signed message says on its page that the signature is not kept;
//! an encrypted one says it could not be read. A `.pst` is opened by MailCraft (`import-pst` into a scratch mailbox,
//! then `export` folder by folder).

use std::borrow::Cow;
use std::fmt::Write as _;
use std::fs;
use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use mail_parser::mailbox::mbox::MessageIterator;
use mail_parser::{Message, MessageParser};

use crate::engine::MAILCRAFT;
use crate::job::{self, Spec};
use crate::markup::{self, Tok};

/// Everything the mail converter takes.
pub const SOURCE_EXTS: &[&str] = &["eml", "mbox", "mbx", "pst"];

/// What a conversion makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One PDF per message file or mailbox (a bookmark per message); one per folder for a `.pst`.
    Pdf,
    /// One `.eml` per message, in a folder.
    Eml,
    /// One mailbox; one per folder for a `.pst`.
    Mbox,
    /// The calendar of a `.pst`.
    Ics,
    /// The contacts of a `.pst`.
    Vcf,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Pdf => "pdf",
            Kind::Eml => "eml",
            Kind::Mbox => "mbox",
            Kind::Ics => "ics",
            Kind::Vcf => "vcf",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Kind> {
        [Kind::Pdf, Kind::Eml, Kind::Mbox, Kind::Ics, Kind::Vcf].into_iter().find(|k| k.ext() == ext)
    }
}

/// How many messages are laid out at the same time (one WordCraft each).
const PARALLEL: usize = 4;

fn lower_ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

fn is_mbox(ext: &str) -> bool {
    matches!(ext, "mbox" | "mbx")
}

/// English text when the language catalog has no entry for `key` (the hub and the command line have none).
fn say(key: &str, english: &str, args: &[(&str, &str)]) -> String {
    let text = crate::tf(key, args);
    if text != key {
        return text;
    }
    let mut out = english.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// One message as a printable page

/// What the page of one message had to leave out or warn about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub remote_pictures: usize,
    pub inline_pictures: usize,
    pub hidden_blocks: usize,
    pub layout_tables: usize,
    pub data_tables: usize,
    pub attachments: Vec<String>,
    pub signed: bool,
    pub encrypted: bool,
}

/// A short line for a bookmark or a file name: `2026-11-13 Subject`.
pub fn title_of(msg: &Message) -> String {
    let subject = msg.subject().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("(no subject)");
    match msg.date() {
        Some(d) => format!("{:04}-{:02}-{:02} {subject}", d.year, d.month, d.day),
        None => subject.to_owned(),
    }
}

fn addresses(list: Option<mail_parser::AddressList>) -> String {
    let Some(list) = list else { return String::new() };
    list.mailboxes()
        .map(|m| match (m.name(), m.address()) {
            (Some(n), Some(a)) if !n.is_empty() && n != a => format!("{n} <{a}>"),
            (_, Some(a)) => a.to_owned(),
            (Some(n), None) => n.to_owned(),
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn date_line(msg: &Message) -> String {
    msg.date()
        .map(|d| {
            let sign = if d.tz_before_gmt { '-' } else { '+' };
            format!("{:04}-{:02}-{:02} {:02}:{:02} (UTC{sign}{:02}:{:02})", d.year, d.month, d.day, d.hour, d.minute, d.tz_hour, d.tz_minute)
        })
        .unwrap_or_default()
}

fn ctype_of(part: &mail_parser::MessagePart) -> (String, String) {
    part.content_type()
        .map(|c| (c.ctype().to_ascii_lowercase(), c.subtype().unwrap_or("").to_ascii_lowercase()))
        .unwrap_or_default()
}

/// Signed / encrypted, from the MIME structure (S/MIME and OpenPGP).
fn security(msg: &Message) -> (bool, bool) {
    let (mut signed, mut encrypted) = (false, false);
    for part in msg.parts() {
        let (t, s) = ctype_of(&part);
        let smime = part.content_type().and_then(|c| c.attribute("smime-type").map(str::to_ascii_lowercase)).unwrap_or_default();
        match (t.as_str(), s.as_str()) {
            ("multipart", "signed") | ("application", "pkcs7-signature" | "x-pkcs7-signature" | "pgp-signature") => signed = true,
            ("multipart", "encrypted") | ("application", "pgp-encrypted") => encrypted = true,
            ("application", "pkcs7-mime" | "x-pkcs7-mime") => {
                if smime.contains("signed") {
                    signed = true;
                } else {
                    // enveloped-data, or no smime-type (old clients): the content is not readable here
                    encrypted = true;
                }
            }
            _ => {}
        }
    }
    (signed, encrypted)
}

/// `cid:` targets as data URIs, for the pictures a message carries inside.
fn inline_pictures(msg: &Message) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in msg.parts() {
        let Some(cid) = part.content_id() else { continue };
        let (t, s) = ctype_of(&part);
        if t != "image" {
            continue;
        }
        let data = base64::engine::general_purpose::STANDARD.encode(part.decoded());
        out.push((cid.trim().trim_matches(['<', '>']).to_owned(), format!("data:image/{s};base64,{data}")));
    }
    out
}

fn attachment_names(msg: &Message) -> Vec<String> {
    msg.attachments()
        .filter_map(|part| {
            if let Some(nested) = part.nested() {
                let subject = nested.subject().unwrap_or("message");
                return Some(format!("{subject}.eml"));
            }
            let (t, sub) = ctype_of(&part);
            // the signature of a signed message is said on the page, not listed as a file
            if matches!(sub.as_str(), "pkcs7-signature" | "x-pkcs7-signature" | "pgp-signature") {
                return None;
            }
            // a picture shown in the body with its cid is not an attachment for the reader
            if t == "image" && part.content_id().is_some() && !part.content_disposition().is_some_and(|d| d.is_attachment()) {
                return None;
            }
            let name = part.attachment_name().map(str::to_owned).unwrap_or_else(|| "attachment".to_owned());
            let kb = (part.decoded_len() / 1024).max(1);
            Some(format!("{name} ({kb} KB)"))
        })
        .collect()
}

/// The HTML body when the message has one, else its text as paragraphs.
fn body_html(msg: &Message, rep: &mut Report) -> String {
    let html: Option<Cow<str>> = msg.html_body().find(|p| p.is_content_type("text", "html")).and_then(|p| p.text());
    if let Some(html) = html {
        let pictures = inline_pictures(msg);
        return clean_html(&html, &pictures, rep);
    }
    let text = msg.body_text(0).unwrap_or_default();
    text_to_html(&text)
}

fn text_to_html(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = String::new();
    for para in text.split("\n\n").map(str::trim_end).filter(|p| !p.trim().is_empty()) {
        out.push_str("<p>");
        out.push_str(&markup::escape_text(para).replace('\n', "<br/>"));
        out.push_str("</p>\n");
    }
    out
}

/// The whole printable page of one message, and what it left out.
pub fn message_page(msg: &Message) -> (String, Report) {
    let mut rep = Report::default();
    let (signed, encrypted) = security(msg);
    rep.signed = signed;
    rep.encrypted = encrypted;
    rep.attachments = attachment_names(msg);
    let body = if encrypted && msg.body_text(0).is_none_or(|t| t.trim().is_empty()) { String::new() } else { body_html(msg, &mut rep) };

    let esc = markup::escape_text;
    let subject = msg.subject().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("(no subject)");
    let mut head = String::new();
    let _ = write!(head, "<h2>{}</h2>\n<p>", esc(subject));
    let mut row = |key: &str, english: &str, value: String| {
        if !value.is_empty() {
            let _ = write!(head, "<b>{}</b> {}<br/>", esc(&say(key, english, &[])), esc(&value));
        }
    };
    row("mail.from", "From:", addresses(msg.from()));
    row("mail.to", "To:", addresses(msg.to()));
    row("mail.cc", "Cc:", addresses(msg.cc()));
    row("mail.date", "Date:", date_line(msg));
    head.push_str("</p>\n");
    if !rep.attachments.is_empty() {
        let _ = write!(head, "<p><b>{}</b> {}</p>\n", esc(&say("mail.attachments", "Attachments:", &[])), esc(&rep.attachments.join(", ")));
    }
    let note = |text: String| format!("<p style=\"color:#555555\"><i>{}</i></p>\n", esc(&text));
    if rep.signed {
        head.push_str(&note(say("mail.signed", "This message was digitally signed. The signature is not kept in this PDF.", &[])));
    }
    if rep.encrypted {
        head.push_str(&note(say("mail.encrypted", "This message is encrypted and could not be read here.", &[])));
    }
    if rep.remote_pictures > 0 {
        let n = rep.remote_pictures.to_string();
        head.push_str(&note(say("mail.remote", "{n} picture(s) stored on the internet were not downloaded.", &[("n", &n)])));
    }
    head.push_str("<hr/>\n");
    let page = format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"/><title>{}</title></head>\n<body>\n{head}{body}\n</body></html>\n",
        esc(subject)
    );
    (page, rep)
}

/// Tags dropped together with everything inside them.
fn dropped_with_content(name: &str) -> bool {
    matches!(name, "head" | "title" | "script" | "style" | "noscript" | "template" | "object" | "embed" | "iframe" | "form" | "svg" | "audio" | "video" | "button" | "select" | "textarea")
}

/// Tags dropped, their content kept.
fn dropped_tag(name: &str) -> bool {
    matches!(name, "html" | "body" | "meta" | "link" | "base" | "font" | "center" | "o:p" | "span" | "input" | "label")
        || name.contains(':')
}

fn is_hidden(attrs: &[(String, String)]) -> bool {
    if attrs.iter().any(|(k, _)| k == "hidden") {
        return true;
    }
    let style: String = markup::attr(attrs, "style").unwrap_or("").to_ascii_lowercase().split_whitespace().collect();
    style.contains("display:none") || style.contains("visibility:hidden") || style.contains("mso-hide:all") || style.contains("max-height:0") && style.contains("overflow:hidden")
}

/// Keeps the style declarations that print well; never anything that loads from the network.
fn clean_style(style: &str, flat: bool) -> String {
    let mut out = Vec::new();
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k.starts_with("mso-") || k.starts_with("-webkit-") || k.starts_with("-ms-") || v.to_ascii_lowercase().contains("url(") || v.to_ascii_lowercase().contains("expression(") {
            continue;
        }
        let layout = matches!(
            k.as_str(),
            "width" | "height" | "min-width" | "max-width" | "min-height" | "max-height" | "position" | "float" | "top" | "left" | "right" | "bottom" | "table-layout" | "display" | "overflow" | "line-height"
        );
        if flat && layout {
            continue;
        }
        if matches!(k.as_str(), "color" | "background-color" | "font-weight" | "font-style" | "text-decoration" | "text-align" | "font-size" | "padding" | "margin" | "border")
            || (!flat && layout)
        {
            out.push(format!("{k}:{v}"));
        }
    }
    out.join(";")
}

/// The body of an HTML mail, made safe and printable. Always well-formed: every tag it opens it closes.
pub fn clean_html(html: &str, pictures: &[(String, String)], rep: &mut Report) -> String {
    let toks = markup::tokenize(html);
    let mut out = String::with_capacity(html.len() / 2);
    // (source tag, written tag, is a data table)
    let mut open: Vec<(String, &'static str)> = Vec::new();
    let mut tables: Vec<bool> = Vec::new();
    // while skipping: the tag that started it and how deep it is nested
    let mut skip: Option<(String, usize)> = None;

    for tok in toks {
        if let Some((name, depth)) = &mut skip {
            match &tok {
                Tok::Start { name: n, selfclose: false, .. } if n == name => *depth += 1,
                Tok::End(n) if n == name => {
                    *depth -= 1;
                    if *depth == 0 {
                        skip = None;
                    }
                }
                _ => {}
            }
            continue;
        }
        match tok {
            Tok::Text(t) => out.push_str(&markup::escape_text(&t)),
            Tok::Start { name, attrs, selfclose } => {
                if dropped_with_content(&name) || is_hidden(&attrs) {
                    if is_hidden(&attrs) && !dropped_with_content(&name) {
                        rep.hidden_blocks += 1;
                    }
                    if !selfclose {
                        skip = Some((name, 1));
                    }
                    continue;
                }
                if name == "img" {
                    out.push_str(&picture(&attrs, pictures, rep));
                    continue;
                }
                if name == "br" || name == "hr" {
                    out.push_str(if name == "br" { "<br/>" } else { "<hr/>" });
                    continue;
                }
                if dropped_tag(&name) || markup::is_void(&name) {
                    continue;
                }
                let in_data = tables.last().copied().unwrap_or(false);
                let written: &'static str = match name.as_str() {
                    "table" => {
                        let border = markup::attr(&attrs, "border").unwrap_or("0").trim();
                        let data = !matches!(border, "" | "0") && markup::attr(&attrs, "role") != Some("presentation");
                        tables.push(data);
                        if data {
                            rep.data_tables += 1;
                            "table"
                        } else {
                            rep.layout_tables += 1;
                            "div"
                        }
                    }
                    "tr" | "td" | "th" | "tbody" | "thead" | "tfoot" if in_data => static_name(&name),
                    "tr" | "td" | "th" => "div",
                    "tbody" | "thead" | "tfoot" | "colgroup" | "col" | "caption" => {
                        if !in_data {
                            continue;
                        }
                        static_name(&name)
                    }
                    other => KEPT.iter().copied().find(|k| *k == other).unwrap_or("div"),
                };
                let mut tag = format!("<{written}");
                let flat = !in_data || written == "div";
                let mut style = markup::attr(&attrs, "style").map(|s| clean_style(s, flat)).unwrap_or_default();
                if let Some(bg) = markup::attr(&attrs, "bgcolor").filter(|v| v.starts_with('#') || v.chars().all(char::is_alphanumeric)) {
                    if !style.contains("background-color") {
                        if !style.is_empty() {
                            style.push(';');
                        }
                        let _ = write!(style, "background-color:{bg}");
                    }
                }
                if !style.is_empty() {
                    let _ = write!(tag, " style=\"{}\"", markup::escape_attr(&style));
                }
                if written == "a" {
                    if let Some(href) = markup::attr(&attrs, "href").filter(|h| safe_link(h)) {
                        let _ = write!(tag, " href=\"{}\"", markup::escape_attr(href));
                    }
                }
                if in_data && matches!(written, "td" | "th") {
                    for k in ["colspan", "rowspan"] {
                        if let Some(v) = markup::attr(&attrs, k).filter(|v| v.chars().all(|c| c.is_ascii_digit())) {
                            let _ = write!(tag, " {k}=\"{v}\"");
                        }
                    }
                }
                if written == "table" {
                    tag.push_str(" border=\"1\" cellpadding=\"4\" style=\"border-collapse:collapse\"");
                }
                tag.push('>');
                if selfclose {
                    // `<div/>` in HTML mail is an empty block; write it closed
                    let _ = write!(tag, "</{written}>");
                    out.push_str(&tag);
                    if name == "table" {
                        tables.pop();
                    }
                    continue;
                }
                out.push_str(&tag);
                open.push((name, written));
            }
            Tok::End(name) => {
                if !open.iter().any(|(n, _)| *n == name) {
                    continue;
                }
                while let Some((n, w)) = open.pop() {
                    let _ = write!(out, "</{w}>");
                    if n == "table" {
                        tables.pop();
                    }
                    if n == name {
                        break;
                    }
                }
            }
        }
    }
    while let Some((n, w)) = open.pop() {
        let _ = write!(out, "</{w}>");
        if n == "table" {
            tables.pop();
        }
    }
    out
}

/// Tags written as they are (anything else becomes a `div`, or goes).
const KEPT: &[&str] = &[
    "p", "div", "a", "b", "strong", "i", "em", "u", "s", "strike", "sub", "sup", "small", "big", "code", "pre", "blockquote", "q", "cite", "h1", "h2", "h3", "h4", "h5", "h6",
    "ul", "ol", "li", "dl", "dt", "dd", "section", "article", "header", "footer", "main", "aside", "nav", "figure", "figcaption", "address", "abbr", "mark", "del", "ins", "kbd", "tt",
];

fn static_name(name: &str) -> &'static str {
    match name {
        "tr" => "tr",
        "td" => "td",
        "th" => "th",
        "tbody" => "tbody",
        "thead" => "thead",
        "tfoot" => "tfoot",
        "colgroup" => "colgroup",
        "col" => "col",
        "caption" => "caption",
        _ => "div",
    }
}

fn safe_link(href: &str) -> bool {
    let h = href.trim().to_ascii_lowercase();
    h.starts_with("http://") || h.starts_with("https://") || h.starts_with("mailto:")
}

fn picture(attrs: &[(String, String)], pictures: &[(String, String)], rep: &mut Report) -> String {
    let src = markup::attr(attrs, "src").unwrap_or("").trim();
    let alt = markup::attr(attrs, "alt").unwrap_or("").trim();
    let lower = src.to_ascii_lowercase();
    if let Some(cid) = lower.strip_prefix("cid:").map(|_| &src[4..]) {
        if let Some((_, uri)) = pictures.iter().find(|(id, _)| id.eq_ignore_ascii_case(cid)) {
            rep.inline_pictures += 1;
            return format!("<img src=\"{}\" alt=\"{}\" style=\"max-width:100%\"/>", markup::escape_attr(uri), markup::escape_attr(alt));
        }
        return String::new();
    }
    if lower.starts_with("data:image/") {
        rep.inline_pictures += 1;
        return format!("<img src=\"{}\" alt=\"{}\" style=\"max-width:100%\"/>", markup::escape_attr(src), markup::escape_attr(alt));
    }
    if src.is_empty() {
        return String::new();
    }
    rep.remote_pictures += 1;
    // a tracking pixel has no words: say nothing; a picture with a description keeps its words
    if alt.is_empty() { String::new() } else { format!("<i>[{}]</i> ", markup::escape_text(alt)) }
}

// ---------------------------------------------------------------------------------------------------------------
// Reading messages

/// The messages of an `.eml` or `.mbox` file, raw, in the order stored.
pub fn read_messages(src: &Path) -> Result<Vec<Vec<u8>>, String> {
    let ext = lower_ext(src);
    if is_mbox(&ext) {
        let file = fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
        let mut out = Vec::new();
        for entry in MessageIterator::new(BufReader::new(file)) {
            let entry = entry.map_err(|e| format!("{}: {e}", src.display()))?;
            out.push(entry.unwrap_contents());
        }
        if out.is_empty() {
            return Err(say("mail.empty", "There are no messages in this file.", &[]));
        }
        Ok(out)
    } else {
        Ok(vec![fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?])
    }
}

fn parse(raw: &[u8]) -> Option<Message<'_>> {
    MessageParser::new().parse(raw)
}

/// A file name from a subject: no characters Windows refuses, short enough.
fn safe_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    out = out.trim().trim_end_matches('.').to_owned();
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect::<String>().trim_end().to_owned();
    }
    if out.is_empty() { "message".to_owned() } else { out }
}

/// A mailbox in the `mboxrd` form: a `From ` line before each message, `>From ` quoting in the body.
pub fn write_mbox(messages: &[Vec<u8>], out: &Path) -> Result<(), String> {
    let mut f = std::io::BufWriter::new(fs::File::create(out).map_err(|e| e.to_string())?);
    for raw in messages {
        let date = parse(raw).and_then(|m| m.date()).map(|d| {
            const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            format!("{} {:2} {:02}:{:02}:{:02} {}", MON[(d.month.clamp(1, 12) - 1) as usize], d.day, d.hour, d.minute, d.second, d.year)
        });
        let io = |e: std::io::Error| e.to_string();
        writeln!(f, "From MAILER-DAEMON Thu {}", date.unwrap_or_else(|| "Jan  1 00:00:00 1970".to_owned())).map_err(io)?;
        let text = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(raw);
        for line in text.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let bare = line.iter().position(|&b| b != b'>').map(|i| &line[i..]).unwrap_or(&[]);
            if bare.starts_with(b"From ") {
                f.write_all(b">").map_err(io)?;
            }
            f.write_all(line).map_err(io)?;
            f.write_all(b"\n").map_err(io)?;
        }
        f.write_all(b"\n").map_err(io)?;
    }
    f.flush().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------------------------
// PDF

/// Lays one raw message out as a PDF in `dir` (its own scratch subfolder) with WordCraft.
fn message_pdf(wordcraft: &Path, raw: &[u8], dir: &Path) -> Result<(PathBuf, String), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let msg = parse(raw).ok_or_else(|| say("mail.unreadable", "This file is not an e-mail message SwapCraft can read.", &[]))?;
    let title = title_of(&msg);
    let (page, _) = message_page(&msg);
    let html = dir.join("mail.html");
    fs::write(&html, page).map_err(|e| e.to_string())?;
    let pdf = dir.join("mail.pdf");
    let mut cmd = Command::new(wordcraft);
    cmd.arg("convert").arg(&html).arg(&pdf);
    job::run_command(&mut cmd, dir)?;
    if !pdf.is_file() {
        return Err("the engine finished but wrote no file".to_owned());
    }
    Ok((pdf, title))
}

/// Every message as a PDF page run, merged into `out` with one bookmark per message, in the order stored. Messages
/// that cannot be laid out are skipped and counted; all failing is an error.
fn messages_pdf(wordcraft: &Path, messages: &[Vec<u8>], tmp: &Path, out: &Path) -> Result<usize, String> {
    if wordcraft.as_os_str().is_empty() {
        return Err(say("mail.need_wordcraft", "The WordCraft engine is needed to make PDFs, and it was not found.", &[]));
    }
    if messages.len() == 1 {
        let (pdf, _) = message_pdf(wordcraft, &messages[0], &tmp.join("m0"))?;
        fs::rename(&pdf, out).or_else(|_| fs::copy(&pdf, out).map(|_| ())).map_err(|e| e.to_string())?;
        return Ok(0);
    }
    let results: Arc<Mutex<Vec<Option<Result<(PathBuf, String), String>>>>> = Arc::new(Mutex::new(vec![None; messages.len()]));
    let next = AtomicUsize::new(0);
    let cancel = job::cancel_flag();
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL.min(messages.len()) {
            let (results, next, cancel) = (Arc::clone(&results), &next, cancel.clone());
            scope.spawn(move || {
                if let Some(flag) = &cancel {
                    job::watch_cancel(flag);
                }
                loop {
                    if cancel.as_ref().is_some_and(|f| f.load(Ordering::Relaxed)) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= messages.len() {
                        break;
                    }
                    let r = message_pdf(wordcraft, &messages[i], &tmp.join(format!("m{i}")));
                    results.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    if cancel.as_ref().is_some_and(|f| f.load(Ordering::Relaxed)) {
        return Err(job::CANCELLED.to_owned());
    }
    let results = std::mem::take(&mut *results.lock().unwrap());
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    let mut failed = 0;
    let mut first_error = None;
    for r in results {
        match r {
            Some(Ok((pdf, title))) => parts.push((title, pdf)),
            Some(Err(e)) => {
                failed += 1;
                first_error.get_or_insert(e);
            }
            None => failed += 1,
        }
    }
    if parts.is_empty() {
        return Err(first_error.unwrap_or_else(|| "no message could be laid out".to_owned()));
    }
    let refs: Vec<(String, &Path)> = parts.iter().map(|(t, p)| (t.clone(), p.as_path())).collect();
    crate::pdfmerge::merge_with_bookmarks(&refs, out)?;
    Ok(failed)
}

// ---------------------------------------------------------------------------------------------------------------
// Outlook data files, through MailCraft

/// One folder of mail in the scratch mailbox.
struct PstFolder {
    /// `Inbox`, `Inbox/Projects`: inside the folder named after the `.pst`.
    path: String,
    id: String,
}

/// The name the `.pst` gets in the scratch folder: MailCraft puts its mail in a folder with this name.
const IMPORT_NAME: &str = "swapcraft_import";

fn mailcraft() -> Result<PathBuf, String> {
    MAILCRAFT.find().ok_or_else(|| say("mail.need_mailcraft", "The MailCraft engine is needed to open Outlook data files (.pst), and it was not found.", &[]))
}

/// Imports the `.pst` into `tmp/box.mailcraft`; returns the mailbox and its folders that hold mail.
/// Where the password of a PST goes: MailCraft reads it from this variable, never from its command line.
pub const PST_PASSWORD_ENV: &str = "MAILCRAFT_PST_PASSWORD";

fn pst_password_missing() -> String {
    say("mail.pst_password", "This Outlook file is protected by a password.", &[])
}

fn pst_password_wrong() -> String {
    say("mail.pst_password_wrong", "That is not the password of this Outlook file.", &[])
}

/// For the app: `Some(false)` when a file failed because its password is missing, `Some(true)` when the password
/// given was wrong, `None` for any other error.
pub fn password_error(msg: &str) -> Option<bool> {
    if msg == pst_password_missing() {
        Some(false)
    } else if msg == pst_password_wrong() {
        Some(true)
    } else {
        None
    }
}

fn open_pst(cli: &Path, src: &Path, tmp: &Path, env: &[(String, String)]) -> Result<(PathBuf, Vec<PstFolder>), String> {
    let mailbox = tmp.join("box.mailcraft");
    // a plain name: MailCraft names the import folder after the file, and accents never reach the engine
    let input = tmp.join(format!("{IMPORT_NAME}.pst"));
    if fs::hard_link(src, &input).is_err() {
        fs::copy(src, &input).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    let mut cmd = Command::new(cli);
    cmd.arg("import-pst").arg(&mailbox).arg(&input);
    cmd.envs(env.iter().map(|(k, v)| (k, v)));
    let imported = job::run_command_for(&mut cmd, tmp, std::time::Duration::from_secs(60 * 60));
    let _ = fs::remove_file(&input);
    if imported.is_err() {
        // MailCraft 0.13: "this .pst file has a password: give it ..." / "that isn't the password of this .pst file ..."
        let said = fs::read_to_string(tmp.join("stderr.txt")).unwrap_or_default();
        if said.contains("isn't the password") {
            return Err(pst_password_wrong());
        }
        if said.contains("has a password") {
            return Err(pst_password_missing());
        }
    }
    imported?;
    if !mailbox.is_file() {
        return Err("MailCraft finished but wrote no mailbox".to_owned());
    }
    let mut cmd = Command::new(cli);
    cmd.arg("folders").arg(&mailbox);
    job::run_command(&mut cmd, tmp)?;
    let json = fs::read_to_string(tmp.join("stdout.txt")).map_err(|e| e.to_string())?;
    let tree: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("MailCraft folders: {e}"))?;
    Ok((mailbox, pst_folders(&tree)))
}

/// From MailCraft's `folders` (`{"accounts":[{"folders":[{"id","name","messages","folders":[..]}]}]}`): the folders
/// under the import folder that hold messages, in the tree's order.
fn pst_folders(tree: &serde_json::Value) -> Vec<PstFolder> {
    fn walk(list: &serde_json::Value, path: &str, out: &mut Vec<PstFolder>) {
        for f in list.as_array().into_iter().flatten() {
            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let here = if path.is_empty() { name.to_owned() } else { format!("{path}/{name}") };
            let count = f.get("messages").and_then(|v| v.as_u64()).unwrap_or(0);
            let id = f.get("id").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_default();
            if let Some(inside) = here.strip_prefix(IMPORT_NAME).and_then(|r| r.strip_prefix('/')) {
                if count > 0 && !id.is_empty() {
                    out.push(PstFolder { path: inside.to_owned(), id });
                }
            }
            if let Some(children) = f.get("folders") {
                walk(children, &here, out);
            }
        }
    }
    let mut out = Vec::new();
    for account in tree.get("accounts").and_then(|a| a.as_array()).into_iter().flatten() {
        if let Some(folders) = account.get("folders") {
            walk(folders, "", &mut out);
        }
    }
    out
}

/// One folder of the scratch mailbox as a mailbox file.
fn export_folder(cli: &Path, mailbox: &Path, folder: &PstFolder, tmp: &Path, n: usize) -> Result<PathBuf, String> {
    let out = tmp.join(format!("f{n}.mbox"));
    let mut cmd = Command::new(cli);
    cmd.arg("export").arg(mailbox).arg(&out).arg("--folder").arg(&folder.id);
    job::run_command_for(&mut cmd, tmp, std::time::Duration::from_secs(30 * 60))?;
    // MailCraft says "wrote" even for an empty file when the folder is wrong: check
    match fs::metadata(&out) {
        Ok(m) if m.len() > 0 => Ok(out),
        _ => Err(format!("MailCraft exported nothing from the folder {}", folder.path)),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The conversion

/// Converts one file. `spec.cli` is WordCraft (only needed for PDF); MailCraft is found on its own for `.pst`.
/// What it returns is a file, or a folder when the result is several files (a folder of messages, one PDF per
/// `.pst` folder).
pub fn convert(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let kind = Kind::from_ext(&spec.out_ext).ok_or_else(|| format!("the mail converter does not make .{} files", spec.out_ext))?;
    let ext = lower_ext(src);
    if !SOURCE_EXTS.contains(&ext.as_str()) {
        return Err(format!("the mail converter does not read .{ext} files"));
    }
    if ext == kind.ext() || (is_mbox(&ext) && kind == Kind::Mbox) {
        return Err(format!("this is already a .{} file", kind.ext()));
    }
    if matches!(kind, Kind::Ics | Kind::Vcf) && ext != "pst" {
        return Err(say("mail.only_pst", "Calendars and contacts come only from Outlook data files (.pst).", &[]));
    }
    let tmp = job::scratch_dir().map_err(|e| e.to_string())?;
    let result = if ext == "pst" { pst_in(&tmp, kind, spec, src, out_dir) } else { file_in(&tmp, kind, spec, src, out_dir) };
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn stem_of(src: &Path) -> String {
    src.file_stem().and_then(|s| s.to_str()).unwrap_or("mail").to_owned()
}

/// `.eml` and `.mbox` sources.
fn file_in(tmp: &Path, kind: Kind, spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let messages = read_messages(src)?;
    let stem = stem_of(src);
    match kind {
        Kind::Pdf => {
            let target = tmp.join("out.pdf");
            messages_pdf(&spec.cli, &messages, &tmp.join("pages"), &target)?;
            job::deliver(&target, out_dir, &format!("{stem}.pdf"))
        }
        Kind::Mbox => {
            let target = tmp.join("out.mbox");
            write_mbox(&messages, &target)?;
            job::deliver(&target, out_dir, &format!("{stem}.mbox"))
        }
        Kind::Eml => {
            let folder = tmp.join("out");
            write_emls(&messages, &folder)?;
            deliver_dir(&folder, out_dir, &stem)
        }
        Kind::Ics | Kind::Vcf => unreachable!("checked in convert"),
    }
}

/// `0001 2026-11-13 Subject.eml` for each message.
fn write_emls(messages: &[Vec<u8>], folder: &Path) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let width = messages.len().to_string().len().max(3);
    for (i, raw) in messages.iter().enumerate() {
        let title = parse(raw).map(|m| title_of(&m)).unwrap_or_else(|| "message".to_owned());
        let name = format!("{:0width$} {}.eml", i + 1, safe_name(&title));
        fs::write(folder.join(name), raw).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pst_in(tmp: &Path, kind: Kind, spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let cli = mailcraft()?;
    let stem = stem_of(src);
    let (mailbox, folders) = open_pst(&cli, src, tmp, &spec.env)?;
    if matches!(kind, Kind::Ics | Kind::Vcf) {
        let target = tmp.join(format!("out.{}", kind.ext()));
        let mut cmd = Command::new(&cli);
        cmd.arg("export").arg(&mailbox).arg(&target);
        let exported = job::run_command(&mut cmd, tmp);
        // MailCraft writes an empty VCALENDAR when there is nothing, and refuses an empty VCF with an error
        let text = fs::read_to_string(&target).unwrap_or_default();
        let empty = if kind == Kind::Ics { !["BEGIN:VEVENT", "BEGIN:VTODO", "BEGIN:VJOURNAL"].iter().any(|b| text.contains(b)) } else { !text.contains("BEGIN:VCARD") };
        if empty {
            return Err(if kind == Kind::Ics {
                say("mail.no_calendar", "This Outlook file has no calendar items.", &[])
            } else {
                say("mail.no_contacts", "This Outlook file has no contacts.", &[])
            });
        }
        exported?;
        return job::deliver(&target, out_dir, &format!("{stem}.{}", kind.ext()));
    }
    if folders.is_empty() {
        return Err(say("mail.empty", "There are no messages in this file.", &[]));
    }
    let result = tmp.join("out");
    fs::create_dir_all(&result).map_err(|e| e.to_string())?;
    for (n, folder) in folders.iter().enumerate() {
        let mbox = export_folder(&cli, &mailbox, folder, tmp, n)?;
        // `Inbox/Projects` -> `Inbox - Projects`, a name for one file or folder
        let name = folder.path.split('/').map(safe_name).collect::<Vec<_>>().join(" - ");
        match kind {
            Kind::Mbox => fs::rename(&mbox, job::unique_path(&result.join(format!("{name}.mbox")))).map_err(|e| e.to_string())?,
            Kind::Eml => write_emls(&read_messages(&mbox)?, &job::unique_path(&result.join(&name)))?,
            Kind::Pdf => {
                let messages = read_messages(&mbox)?;
                messages_pdf(&spec.cli, &messages, &tmp.join(format!("pages{n}")), &job::unique_path(&result.join(format!("{name}.pdf"))))?;
                let _ = fs::remove_dir_all(tmp.join(format!("pages{n}")));
            }
            Kind::Ics | Kind::Vcf => unreachable!(),
        }
        let _ = fs::remove_file(&mbox);
    }
    deliver_dir(&result, out_dir, &stem)
}

/// Moves a finished folder to `out_dir/<name>` (or `<name> (2)`), copying when it is on another drive.
fn deliver_dir(folder: &Path, out_dir: &Path, name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let target = job::unique_path(&out_dir.join(name));
    if fs::rename(folder, &target).is_err() {
        copy_dir(folder, &target).map_err(|e| e.to_string())?;
    }
    Ok(target)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_of(raw: &str) -> (String, Report) {
        let msg = parse(raw.as_bytes()).expect("parses");
        message_page(&msg)
    }

    #[test]
    fn plain_text_with_accents_in_quoted_printable() {
        let (page, rep) = page_of(
            "From: Giulia Bianchi <giulia@example.com>\r\nTo: Marco <marco@example.com>\r\nSubject: =?UTF-8?Q?Riunione_di_venerd=C3=AC?=\r\nDate: Thu, 13 Nov 2026 10:15:00 +0100\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nCiao, ecco l'ordine del giorno perch=C3=A9 in citt=C3=A0 <tutti>.\r\n\r\nSecondo paragrafo.\r\n",
        );
        assert!(page.contains("Riunione di venerd\u{ec}"), "{page}");
        assert!(page.contains("Giulia Bianchi &lt;giulia@example.com&gt;"));
        assert!(page.contains("perch\u{e9} in citt\u{e0} &lt;tutti&gt;."), "text is escaped: {page}");
        assert!(page.contains("2026-11-13 10:15 (UTC+01:00)"));
        assert_eq!(page.matches("<p>").count(), 3, "header + two paragraphs: {page}");
        assert!(!rep.signed && !rep.encrypted);
    }

    #[test]
    fn html_is_cleaned_remote_pictures_never_load_and_cid_pictures_are_drawn() {
        let raw = concat!(
            "From: shop@example.com\r\nSubject: Offerta\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n",
            "--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n",
            "<html><head><style>.x{color:red}</style><script>alert('PWNED_SCRIPT')</script></head><body>",
            "<table border=\"0\" width=\"600\"><tr><td style=\"width:600px;color:#333\">Offerta valida fino a domenica",
            "<img src=\"https://tracker.example.com/p.gif\" width=\"1\"><img src=\"https://cdn.example.com/a.png\" alt=\"Logo del negozio\">",
            "<img src=\"cid:logo@x\"></td></tr></table>",
            "<div style=\"display:none\">SOLO MOBILE duplicate</div>",
            "<table border=\"1\"><tr><th>Art.</th><th>Prezzo</th></tr><tr><td colspan=\"2\">1.250,00</td></tr></table>",
            "<a href=\"javascript:evil()\">link</a> <a href=\"https://example.com\" onclick=\"x()\">sito</a> Fine.",
            "<div style=\"background:url(https://x.example/bg.png)\">bg</div></body></html>\r\n",
            "--b\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--b--\r\n",
        );
        let (page, rep) = page_of(raw);
        assert!(page.contains("Offerta valida fino a domenica"));
        assert!(page.contains("Fine."));
        assert!(!page.contains("PWNED_SCRIPT") && !page.contains("<script"), "{page}");
        assert!(!page.contains("SOLO MOBILE"), "hidden blocks go");
        assert!(!page.contains("tracker.example.com") && !page.contains("cdn.example.com") && !page.contains("x.example"), "nothing remote: {page}");
        assert!(page.contains("[Logo del negozio]"), "a remote picture's description stays");
        assert!(page.contains("data:image/png;base64,iVBORw0KGgo="), "the cid picture is inside");
        assert!(!page.contains("javascript:") && !page.contains("onclick"));
        assert!(page.contains("href=\"https://example.com\""));
        assert!(page.contains("<td colspan=\"2\">1.250,00</td>"), "data tables keep their cells: {page}");
        assert!(!page.contains("width:600px"), "layout widths go");
        assert_eq!((rep.remote_pictures, rep.inline_pictures, rep.hidden_blocks, rep.layout_tables, rep.data_tables), (2, 1, 1, 1, 1));
        assert!(page.contains("2 picture(s) stored on the internet were not downloaded."));
        // always well-formed: as many opening as closing divs
        assert_eq!(page.matches("<div").count(), page.matches("</div>").count(), "{page}");
    }

    #[test]
    fn broken_html_is_closed() {
        let mut rep = Report::default();
        let out = clean_html("<div><p>uno<b>due</div><table><tr><td>tre", &[], &mut rep);
        assert_eq!(out, "<div><p>uno<b>due</b></p></div><div><div><div>tre</div></div></div>");
    }

    #[test]
    fn signed_encrypted_and_attachments_are_said() {
        let signed = concat!(
            "From: a@example.com\r\nSubject: Firmato\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; boundary=\"s\"\r\n\r\n",
            "--s\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n--m\r\nContent-Type: text/plain\r\n\r\nTesto firmato.\r\n",
            "--m\r\nContent-Type: application/pdf; name=\"fattura-42.pdf\"\r\nContent-Disposition: attachment; filename=\"fattura-42.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--m--\r\n",
            "--s\r\nContent-Type: application/pkcs7-signature; name=smime.p7s\r\n\r\nAAAA\r\n--s--\r\n",
        );
        let (page, rep) = page_of(signed);
        assert!(rep.signed && !rep.encrypted);
        assert!(page.contains("Testo firmato."));
        assert!(page.contains("signature is not kept"));
        assert!(rep.attachments.iter().any(|a| a.starts_with("fattura-42.pdf")), "{:?}", rep.attachments);
        let encrypted = "From: a@example.com\r\nSubject: Segreto\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data; name=smime.p7m\r\nContent-Transfer-Encoding: base64\r\n\r\nMIAGCSqGSIb3DQEHA6CAMIACAQAx\r\n";
        let (page, rep) = page_of(encrypted);
        assert!(rep.encrypted && page.contains("encrypted and could not be read"));
        assert!(!page.contains("MIAGCSq"), "no base64 garbage on the page");
    }

    #[test]
    fn mbox_round_trip_and_eml_folder() {
        let dir = std::env::temp_dir().join(format!("vf_mail_mbox_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = b"From: a@example.com\nSubject: Primo\nDate: Mon, 3 Nov 2026 08:00:00 +0000\n\nFrom the start, a line that looks like a separator.\n".to_vec();
        let b = b"From: b@example.com\nSubject: Secondo: s/n?\n\nCiao.\n".to_vec();
        let mbox = dir.join("box.mbox");
        write_mbox(&[a.clone(), b.clone()], &mbox).unwrap();
        let text = fs::read_to_string(&mbox).unwrap();
        assert!(text.contains("\n>From the start"), "body From quoted: {text}");
        let back = read_messages(&mbox).unwrap();
        assert_eq!(back.len(), 2);
        let first = parse(&back[0]).unwrap();
        assert_eq!(first.subject(), Some("Primo"));
        assert!(first.body_text(0).unwrap().contains("From the start"), "quoting undone on reading");
        let spec = Spec { cli: PathBuf::new(), out_ext: "eml".into(), extra_args: vec![], env: vec![] };
        let folder = convert(&spec, &mbox, &dir.join("out")).unwrap();
        let mut names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["001 2026-11-03 Primo.eml", "002 Secondo_ s_n_.eml"]);
        // eml -> mbox -> the same message
        let one = dir.join("one.eml");
        fs::write(&one, &b).unwrap();
        let spec = Spec { out_ext: "mbox".into(), ..spec };
        let made = convert(&spec, &one, &dir.join("out")).unwrap();
        assert_eq!(read_messages(&made).unwrap().len(), 1);
        assert!(convert(&spec, &mbox, &dir).unwrap_err().contains("already"));
        let pdf_spec = Spec { out_ext: "pdf".into(), ..spec.clone() };
        assert!(convert(&pdf_spec, &one, &dir).unwrap_err().contains("WordCraft"));
        let ics = Spec { out_ext: "ics".into(), ..spec };
        assert!(convert(&ics, &one, &dir).unwrap_err().contains(".pst"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_for_windows() {
        assert_eq!(safe_name("Re: a/b \"c\"?"), "Re_ a_b _c__");
        assert_eq!(safe_name("  ..  "), "message");
        assert_eq!(safe_name(&"x".repeat(200)).len(), 80);
    }

    #[test]
    fn mailcraft_folder_tree() {
        // the shape MailCraft 0.13.0 `folders` prints (checked on a real import)
        let tree: serde_json::Value = serde_json::from_str(
            r#"{"accounts":[{"id":1,"name":"Mailbox","folders":[
                {"id":2,"name":"Inbox","messages":9,"folders":[]},
                {"id":10,"name":"swapcraft_import","messages":0,"folders":[
                    {"id":18,"name":"Inbox","messages":74,"folders":[{"id":30,"name":"Projects","messages":2,"folders":[]}]},
                    {"id":11,"name":"Posta eliminata","messages":0,"folders":[]}]}]}],"current":2}"#,
        )
        .unwrap();
        let got: Vec<(String, String)> = pst_folders(&tree).into_iter().map(|f| (f.path, f.id)).collect();
        assert_eq!(got, [("Inbox".to_owned(), "18".to_owned()), ("Inbox/Projects".to_owned(), "30".to_owned())]);
    }

    /// With the real WordCraft (skipped without it): two messages in a mailbox make one PDF with two bookmarks.
    #[test]
    fn real_engine_mailbox_to_pdf() {
        let Some(cli) = crate::engine::WORDCRAFT.find() else { return };
        let dir = std::env::temp_dir().join(format!("vf_mail_pdf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = "From: Giulia <g@example.com>\nSubject: Caff\u{e8} alle 10\nDate: Mon, 3 Nov 2026 08:00:00 +0100\nContent-Type: text/plain; charset=utf-8\n\nCi vediamo in citt\u{e0}.\n".as_bytes().to_vec();
        let b = b"From: b@example.com\nSubject: Report\nContent-Type: text/html\n\n<table border=0><tr><td>Totale <b>42</b></td></tr></table>\n".to_vec();
        let mbox = dir.join("Posta di prova.mbox");
        write_mbox(&[a, b], &mbox).unwrap();
        let spec = Spec { cli, out_ext: "pdf".into(), extra_args: vec![], env: vec![] };
        let pdf = convert(&spec, &mbox, &dir).expect("mbox -> pdf");
        assert_eq!(pdf.file_name().unwrap(), "Posta di prova.pdf");
        let doc = lopdf::Document::load(&pdf).unwrap();
        assert!(doc.get_pages().len() >= 2, "one page per message at least");
        let _ = fs::remove_dir_all(&dir);
    }
}
