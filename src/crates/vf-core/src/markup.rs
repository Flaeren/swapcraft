//! A small, forgiving HTML/XHTML reader and a strict XHTML writer, for the e-book converters.
//!
//! E-book markup is a mix: EPUB chapters are XHTML (well-formed), Kindle's text is HTML-ish with unclosed tags,
//! unknown prefixes (`mbp:pagebreak`) and fragments that are not whole documents. So nothing here assumes a
//! tree: [`tokenize`] turns any of it into a flat list of tags and text, and the two writers ([`to_text`],
//! [`to_xhtml`]) walk that list, keeping a stack of what is open so their output is always well-formed.

/// One piece of markup. Tag and attribute names are lower case.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Start { name: String, attrs: Vec<(String, String)>, selfclose: bool },
    End(String),
    /// Text with entities already decoded.
    Text(String),
}

/// Elements that never have content.
pub fn is_void(name: &str) -> bool {
    matches!(name, "br" | "hr" | "img" | "meta" | "link" | "input" | "col" | "area" | "base" | "wbr" | "source" | "track" | "embed" | "param")
}

/// Elements that start a new line in plain text.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "ul" | "ol" | "tr" | "table" | "blockquote" | "section" | "article" | "header" | "footer" | "pre" | "figure" | "figcaption" | "dt" | "dd" | "dl" | "hr" | "br" | "aside" | "nav" | "body"
    )
}

/// Elements whose content is never shown.
fn is_hidden(name: &str) -> bool {
    matches!(name, "head" | "title" | "script" | "style" | "template" | "svg")
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "ndash" => "\u{2013}",
        "mdash" => "\u{2014}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "hellip" => "\u{2026}",
        "copy" => "\u{a9}",
        "reg" => "\u{ae}",
        "deg" => "\u{b0}",
        "eacute" => "\u{e9}",
        "egrave" => "\u{e8}",
        "agrave" => "\u{e0}",
        "ugrave" => "\u{f9}",
        "ograve" => "\u{f2}",
        "igrave" => "\u{ec}",
        "ccedil" => "\u{e7}",
        "shy" => "\u{ad}",
        "times" => "\u{d7}",
        "middot" => "\u{b7}",
        "bull" => "\u{2022}",
        "laquo" => "\u{ab}",
        "raquo" => "\u{bb}",
        _ => return None,
    })
}

/// Replaces `&amp;`, `&#233;`, `&#xE9;` and the common named entities. Unknown ones are left as written.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find([';', '&', ' ', '<']).filter(|&e| rest.as_bytes()[1 + e] == b';' && e <= 10);
        let decoded = end.and_then(|e| {
            let body = &rest[1..1 + e];
            let ch = if let Some(num) = body.strip_prefix('#') {
                let code = if let Some(hex) = num.strip_prefix(['x', 'X']) { u32::from_str_radix(hex, 16).ok() } else { num.parse::<u32>().ok() };
                code.and_then(char::from_u32).map(String::from)
            } else {
                named_entity(body).map(str::to_owned)
            };
            ch.map(|c| (c, e + 2))
        });
        match decoded {
            Some((c, used)) => {
                out.push_str(&c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

fn parse_attrs(mut s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if s.is_empty() {
            break;
        }
        let name_end = s.find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>').unwrap_or(s.len());
        if name_end == 0 {
            s = &s[1..];
            continue;
        }
        let name = s[..name_end].to_lowercase();
        s = s[name_end..].trim_start();
        let mut value = String::new();
        if let Some(after) = s.strip_prefix('=') {
            let after = after.trim_start();
            if let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let inner = &after[1..];
                let close = inner.find(q).unwrap_or(inner.len());
                value = decode_entities(&inner[..close]);
                s = &inner[(close + 1).min(inner.len())..];
            } else {
                let end = after.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(after.len());
                value = decode_entities(&after[..end]);
                s = &after[end..];
            }
        }
        attrs.push((name, value));
    }
    attrs
}

/// Splits markup into tags and text. Comments, processing instructions and doctypes are dropped; `<script>` and
/// `<style>` keep only their tags (their content is not markup).
pub fn tokenize(src: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut i = 0;
    let b = src.as_bytes();
    let mut text_start = 0;
    let flush = |toks: &mut Vec<Tok>, from: usize, to: usize| {
        if to > from {
            // soft hyphens are line-breaking hints some books sprinkle through every word; they are not text
            toks.push(Tok::Text(decode_entities(&src[from..to]).replace('\u{ad}', "")));
        }
    };
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &src[i..];
        if let Some(body) = rest.strip_prefix("<!--") {
            flush(&mut toks, text_start, i);
            i += 4 + body.find("-->").map_or(body.len(), |e| e + 3);
            text_start = i;
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            flush(&mut toks, text_start, i);
            let end = body.find("]]>").unwrap_or(body.len());
            toks.push(Tok::Text(body[..end].to_owned()));
            i += 9 + (end + 3).min(body.len());
            text_start = i;
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            flush(&mut toks, text_start, i);
            i += rest.find('>').map_or(rest.len(), |e| e + 1);
            text_start = i;
        } else if rest.len() > 1 && (rest.as_bytes()[1] == b'/' || rest.as_bytes()[1].is_ascii_alphabetic()) {
            let Some(close) = tag_end(rest) else {
                i += 1;
                continue;
            };
            flush(&mut toks, text_start, i);
            let inner = &rest[1..close];
            if let Some(name) = inner.strip_prefix('/') {
                toks.push(Tok::End(name.trim().to_lowercase()));
            } else {
                let name_end = inner.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(inner.len());
                let name = inner[..name_end].to_lowercase();
                let selfclose = inner.ends_with('/') || is_void(&name);
                let raw = matches!(name.as_str(), "script" | "style") && !inner.ends_with('/');
                toks.push(Tok::Start { name: name.clone(), attrs: parse_attrs(&inner[name_end..]), selfclose });
                i += close + 1;
                if raw {
                    // everything up to the matching close tag is code, not markup
                    let lower = src[i..].to_lowercase();
                    let end = lower.find(&format!("</{name}")).unwrap_or(lower.len());
                    i += end;
                }
                text_start = i;
                continue;
            }
            i += close + 1;
            text_start = i;
        } else {
            i += 1;
        }
    }
    flush(&mut toks, text_start, b.len());
    toks
}

/// The index of the `>` that ends the tag starting at `s[0] == '<'`, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, &c) in s.as_bytes().iter().enumerate().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(c),
            (None, b'>') => return Some(i),
            (None, b'<') => return None,
            _ => {}
        }
    }
    None
}

/// The value of attribute `name` of a start tag.
pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Readable plain text: blocks on their own lines, one blank line between paragraphs, no markup.
pub fn to_text(toks: &[Tok]) -> String {
    let mut out = String::new();
    let mut hidden = 0usize;
    let mut pre = 0usize;
    let newline = |out: &mut String, blank: bool| {
        let trimmed = out.trim_end_matches([' ', '\t']).len();
        out.truncate(trimmed);
        if out.is_empty() {
            return;
        }
        let want = if blank { "\n\n" } else { "\n" };
        let have = out.chars().rev().take_while(|c| *c == '\n').count();
        for _ in have..want.len() {
            out.push('\n');
        }
    };
    for t in toks {
        match t {
            Tok::Start { name, selfclose, .. } => {
                if is_hidden(name) && !selfclose {
                    hidden += 1;
                } else if hidden == 0 {
                    if name == "pre" {
                        pre += 1;
                    }
                    if name == "li" {
                        newline(&mut out, false);
                        out.push_str("- ");
                    } else if name == "br" {
                        newline(&mut out, false);
                    } else if is_block(name) {
                        newline(&mut out, matches!(name.as_str(), "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre"));
                    }
                }
            }
            Tok::End(name) => {
                if is_hidden(name) {
                    hidden = hidden.saturating_sub(1);
                } else if hidden == 0 {
                    if name == "pre" {
                        pre = pre.saturating_sub(1);
                    }
                    if is_block(name) && !matches!(name.as_str(), "li" | "ul" | "ol") {
                        newline(&mut out, matches!(name.as_str(), "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre"));
                    } else if matches!(name.as_str(), "li" | "ul" | "ol") {
                        newline(&mut out, false);
                    }
                }
            }
            Tok::Text(s) if hidden == 0 => {
                if pre > 0 {
                    out.push_str(s);
                } else {
                    // collapse white space like a browser does
                    let mut last_space = out.ends_with([' ', '\n']) || out.is_empty();
                    for c in s.chars() {
                        if c.is_whitespace() && c != '\u{a0}' {
                            if !last_space {
                                out.push(' ');
                                last_space = true;
                            }
                        } else {
                            out.push(if c == '\u{a0}' { ' ' } else { c });
                            last_space = false;
                        }
                    }
                }
            }
            Tok::Text(_) => {}
        }
    }
    out.trim().to_owned() + "\n"
}

const KEEP_ATTRS: &[&str] = &["href", "src", "alt", "id", "title", "colspan", "rowspan", "lang"];

/// Well-formed XHTML for the body content of `toks`: the document wrapper (`html`, `head`, `body`) and anything
/// hidden (`title`, `style`, `script`) is dropped, prefixed tags (`mbp:pagebreak`) are dropped but their content
/// kept, only a few attributes survive, every tag is closed, text is escaped. `src` is passed through
/// `rewrite_src`, which may return a new value (an image moved into the book) or `None` to drop the image.
pub fn to_xhtml(toks: &[Tok], rewrite_src: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut open: Vec<String> = Vec::new();
    let mut hidden = 0usize;
    for t in toks {
        match t {
            Tok::Start { name, attrs, selfclose } => {
                if is_hidden(name) {
                    if !selfclose {
                        hidden += 1;
                    }
                    continue;
                }
                if hidden > 0 || matches!(name.as_str(), "html" | "body") || name.contains(':') || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
                    continue;
                }
                let mut tag = format!("<{name}");
                let mut drop_tag = false;
                for (k, v) in attrs {
                    if !KEEP_ATTRS.contains(&k.as_str()) {
                        continue;
                    }
                    if k == "src" {
                        match rewrite_src(v) {
                            Some(new) => tag.push_str(&format!(" src=\"{}\"", escape_attr(&new))),
                            None => drop_tag = true,
                        }
                    } else if k == "href" && (v.starts_with("kindle:") || v.starts_with("javascript:")) {
                        // a Kindle position or script: nothing to point at once the book is converted
                    } else {
                        tag.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
                    }
                }
                if drop_tag {
                    continue;
                }
                if name == "img" && !tag.contains(" alt=") {
                    tag.push_str(" alt=\"\"");
                }
                if *selfclose || is_void(name) {
                    tag.push_str("/>");
                } else {
                    open.push(name.clone());
                    tag.push('>');
                }
                out.push_str(&tag);
            }
            Tok::End(name) => {
                if is_hidden(name) {
                    hidden = hidden.saturating_sub(1);
                } else if hidden == 0 && open.contains(name) {
                    // close what was left open inside it, then it
                    while let Some(top) = open.pop() {
                        out.push_str(&format!("</{top}>"));
                        if &top == name {
                            break;
                        }
                    }
                }
            }
            Tok::Text(s) if hidden == 0 => out.push_str(&escape_text(s)),
            Tok::Text(_) => {}
        }
    }
    while let Some(top) = open.pop() {
        out.push_str(&format!("</{top}>"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_are_decoded() {
        assert_eq!(decode_entities("Caff&egrave; &amp; t&#232; &#x2014; &unknown; & fine"), "Caff\u{e8} & t\u{e8} \u{2014} &unknown; & fine");
    }

    #[test]
    fn plain_text_has_paragraphs_and_no_markup() {
        let toks = tokenize("<html><head><title>T</title><style>p{}</style></head><body><h1>Titolo</h1><p>Uno   due\n tre.</p><p>Quattro<br>cinque</p><ul><li>a</li><li>b</li></ul></body></html>");
        assert_eq!(to_text(&toks), "Titolo\n\nUno due tre.\n\nQuattro\ncinque\n\n- a\n- b\n");
    }

    #[test]
    fn broken_html_becomes_well_formed_xhtml() {
        let toks = tokenize("<p>Un <b>grassetto <i>annidato</p><p class='x'>due<br>righe<img src=\"a.png\"><mbp:pagebreak/></p><!-- c --><script>if (a<b) {}</script>fine");
        let mut seen = Vec::new();
        let x = to_xhtml(&toks, &mut |s| {
            seen.push(s.to_owned());
            Some(format!("images/{s}"))
        });
        assert_eq!(x, "<p>Un <b>grassetto <i>annidato</i></b></p><p>due<br/>righe<img src=\"images/a.png\" alt=\"\"/></p>fine");
        assert_eq!(seen, ["a.png"]);
    }

    #[test]
    fn kindle_links_and_missing_images_are_dropped() {
        let toks = tokenize("<p><a href=\"kindle:pos:fid:0001:off:0\">x</a><img src=\"kindle:embed:0002\"/></p>");
        let x = to_xhtml(&toks, &mut |_| None);
        assert_eq!(x, "<p><a>x</a></p>");
    }

    #[test]
    fn quoted_greater_than_does_not_end_a_tag() {
        let toks = tokenize("<a title=\"a > b\" href='x'>t</a>");
        assert_eq!(toks[0], Tok::Start { name: "a".into(), attrs: vec![("title".into(), "a > b".into()), ("href".into(), "x".into())], selfclose: false });
    }
}
