//! Kindle files without DRM: AZW3 (KF8), AZW, MOBI and PRC, read into a [`Book`].
//!
//! The container is a Palm database: a list of records, the first of which holds the PalmDOC and MOBI headers
//! (compression, encoding, where the pictures start), then the compressed text, then indexes and pictures. A KF8
//! (AZW3) file keeps the book's HTML in the text records, one "skeleton" per file followed by its fragments, plus
//! further flows (style sheets) after it; the table of flows (FDST) says where the HTML ends. The indexes that
//! say exactly where each fragment goes are not read: skeleton and fragments are taken in the order they are stored,
//! which keeps every word of the text in order and one chapter per file, at the price of some nesting.
//!
//! Protected books (the encryption field is not 0) are refused with a clear message. Nothing is decrypted.

use crate::epub::{Book, Chapter, DRM_MESSAGE, Images, sniff_mime};
use crate::markup::{self, Tok, tokenize};

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// PalmDOC "LZ77": literals, copies from earlier output, and a space fused with the next character.
pub fn palmdoc_decompress(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(data.len() * 3);
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        i += 1;
        match c {
            0 | 9..=0x7f => out.push(c),
            1..=8 => {
                let n = (c as usize).min(data.len() - i);
                out.extend_from_slice(&data[i..i + n]);
                i += n;
            }
            0x80..=0xbf => {
                let Some(&next) = data.get(i) else { break };
                i += 1;
                let pair = (c as usize) << 8 | next as usize;
                let (dist, len) = ((pair >> 3) & 0x7ff, (pair & 7) + 3);
                if dist == 0 || dist > out.len() {
                    continue;
                }
                for _ in 0..len {
                    out.push(out[out.len() - dist]);
                }
            }
            0xc0..=0xff => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
    out
}

/// Bytes at the end of a text record that belong to the format, not to the text (see the MOBI header's
/// "extra data flags").
fn trailing_len(rec: &[u8], flags: u16) -> usize {
    let mut total = 0usize;
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            // a size written backwards: 7 bits per byte, the byte with the top bit set ends it
            let (mut size, mut shift) = (0usize, 0);
            let mut end = rec.len().saturating_sub(total);
            while end > 0 {
                let b = rec[end - 1];
                size |= ((b & 0x7f) as usize) << shift;
                shift += 7;
                end -= 1;
                if b & 0x80 != 0 || shift >= 28 {
                    break;
                }
            }
            total += size;
        }
        f >>= 1;
    }
    if flags & 1 != 0 {
        let at = rec.len().saturating_sub(total + 1);
        total += (rec.get(at).copied().unwrap_or(0) & 3) as usize + 1;
    }
    total.min(rec.len())
}

const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', ' ', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', ' ', '\u{17d}', ' ', ' ', '\u{2018}', '\u{2019}', '\u{201c}',
    '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', ' ', '\u{17e}', '\u{178}',
];

fn decode_text(bytes: &[u8], encoding: u32) -> String {
    if encoding == 65001 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        bytes.iter().map(|&b| if (0x80..0xa0).contains(&b) { CP1252_HIGH[(b - 0x80) as usize] } else { b as char }).collect()
    }
}

/// Reads a Kindle file's bytes.
pub fn read_mobi(data: &[u8]) -> Result<Book, String> {
    if data.len() < 100 || &data[60..68] != b"BOOKMOBI" && &data[60..68] != b"TEXtREAd" {
        return Err("not a Kindle file (no MOBI header)".to_owned());
    }
    let n = u16_at(data, 76).ok_or("damaged file")? as usize;
    let mut offsets: Vec<usize> = (0..n).filter_map(|i| u32_at(data, 78 + 8 * i).map(|o| o as usize)).collect();
    if offsets.len() != n || n < 2 {
        return Err("damaged file (record list)".to_owned());
    }
    offsets.push(data.len());
    let record = |i: usize| -> &[u8] {
        let (a, b) = (offsets[i].min(data.len()), offsets[i + 1].min(data.len()));
        if a <= b { &data[a..b] } else { &[] }
    };
    let r0 = record(0);
    let compression = u16_at(r0, 0).ok_or("damaged file (header)")?;
    let text_records = u16_at(r0, 8).ok_or("damaged file (header)")? as usize;
    let encryption = u16_at(r0, 12).ok_or("damaged file (header)")?;
    if encryption != 0 {
        return Err(DRM_MESSAGE.to_owned());
    }
    if r0.get(16..20) != Some(b"MOBI") {
        return Err("this old Palm book has no MOBI header".to_owned());
    }
    match compression {
        1 | 2 => {}
        17480 => return Err("this Kindle file uses Huffman compression, which is not supported".to_owned()),
        other => return Err(format!("unknown compression {other}")),
    }
    let header_len = u32_at(r0, 20).unwrap_or(0) as usize;
    let encoding = u32_at(r0, 28).unwrap_or(65001);
    let version = u32_at(r0, 36).unwrap_or(0);
    let first_image = u32_at(r0, 0x6c).unwrap_or(0xffff_ffff) as usize;
    let exth_flags = u32_at(r0, 0x80).unwrap_or(0);
    let extra_flags = if header_len >= 0xe4 { u16_at(r0, 0xf2).unwrap_or(0) } else { 0 };

    // the text
    let mut text: Vec<u8> = Vec::new();
    for i in 1..=text_records.min(n - 1) {
        let rec = record(i);
        let rec = &rec[..rec.len() - trailing_len(rec, extra_flags)];
        if compression == 2 {
            text.extend(palmdoc_decompress(rec));
        } else {
            text.extend_from_slice(rec);
        }
    }
    // KF8: the HTML is the first flow; the others are style sheets and the like
    if version >= 8 {
        let fdst = u32_at(r0, 0xc0).unwrap_or(0xffff_ffff) as usize;
        if fdst < n && record(fdst).starts_with(b"FDST") {
            let f = record(fdst);
            let (start, end) = (u32_at(f, 12), u32_at(f, 16));
            if let (Some(s), Some(e)) = (start, end)
                && (s as usize) < (e as usize)
                && (e as usize) <= text.len()
            {
                text = text[s as usize..e as usize].to_vec();
            }
        }
    }
    let html = decode_text(&text, encoding);

    // title, authors, language: EXTH records, else the full name
    let mut book = Book { language: "en".into(), ..Book::default() };
    if exth_flags & 0x40 != 0 {
        let mut p = 16 + header_len;
        if r0.get(p..p + 4) == Some(b"EXTH") {
            let count = u32_at(r0, p + 8).unwrap_or(0);
            p += 12;
            for _ in 0..count {
                let (Some(kind), Some(len)) = (u32_at(r0, p), u32_at(r0, p + 4)) else { break };
                let len = len as usize;
                if len < 8 || p + len > r0.len() {
                    break;
                }
                let value = String::from_utf8_lossy(&r0[p + 8..p + len]).trim().to_owned();
                match kind {
                    100 if !value.is_empty() => book.authors.push(value),
                    503 if !value.is_empty() => book.title = value,
                    524 if !value.is_empty() => book.language = value,
                    _ => {}
                }
                p += len;
            }
        }
    }
    if book.title.is_empty() {
        let (off, len) = (u32_at(r0, 0x54).unwrap_or(0) as usize, u32_at(r0, 0x58).unwrap_or(0) as usize);
        book.title = r0.get(off..off + len).map(|b| decode_text(b, encoding).trim().to_owned()).unwrap_or_default();
    }
    if book.title.is_empty() {
        book.title = String::from_utf8_lossy(&data[..32]).trim_end_matches('\0').replace('_', " ");
    }

    // chapters: one per KF8 file (skeleton), or per page break in an old MOBI
    let lower = html.to_lowercase();
    let marker = if version >= 8 { "<html" } else { "<mbp:pagebreak" };
    let mut cuts: Vec<usize> = lower.match_indices(marker).map(|(i, _)| i).collect();
    if cuts.first() != Some(&0) {
        cuts.insert(0, 0);
    }
    cuts.push(html.len());
    let mut images = Images::default();
    let mut image_for = |reference: &str| -> Option<String> {
        // "kindle:embed:0004?mime=..." (base 32), or "mobi:rec:12" made from an old <img recindex="00012">
        let idx = if let Some(rest) = reference.strip_prefix("kindle:embed:") {
            usize::from_str_radix(rest.split('?').next()?, 32).ok()?
        } else {
            reference.strip_prefix("mobi:rec:")?.parse::<usize>().ok()?
        };
        let rec = first_image.checked_add(idx.checked_sub(1)?)?;
        if rec >= n {
            return None;
        }
        let bytes = record(rec);
        sniff_mime(bytes)?;
        images.add(&format!("rec{rec}"), bytes.to_vec())
    };
    for w in cuts.windows(2) {
        let mut toks = tokenize(&html[w[0]..w[1]]);
        // old MOBI pictures are <img recindex="N">
        for t in &mut toks {
            if let Tok::Start { name, attrs, .. } = t
                && name == "img"
                && !attrs.iter().any(|(k, _)| k == "src")
                && let Some((_, v)) = attrs.iter().find(|(k, _)| k == "recindex").cloned()
                && let Ok(idx) = v.trim().parse::<usize>()
            {
                attrs.push(("src".into(), format!("mobi:rec:{idx}")));
            }
        }
        let body = markup::to_xhtml(&toks, &mut image_for);
        if body.trim().is_empty() || !body.contains(|c: char| c.is_alphanumeric()) && !body.contains("<img") {
            continue;
        }
        let title = chapter_name(&toks).unwrap_or_else(|| format!("Chapter {}", book.chapters.len() + 1));
        book.chapters.push(Chapter { title, body });
    }
    book.images = images.list;
    if book.chapters.is_empty() {
        return Err("this Kindle file has no readable text".to_owned());
    }
    Ok(book)
}

/// A chapter's name: its first heading.
fn chapter_name(toks: &[Tok]) -> Option<String> {
    let i = toks.iter().position(|t| matches!(t, Tok::Start { name, .. } if matches!(name.as_str(), "h1" | "h2" | "h3")))?;
    let mut depth = 0usize;
    let mut s = String::new();
    for t in &toks[i..] {
        match t {
            Tok::Start { selfclose: false, .. } => depth += 1,
            Tok::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Tok::Text(x) => s.push_str(x),
            _ => {}
        }
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palmdoc_literals_copies_and_spaces() {
        // "Hello " literal, then a copy of 3 bytes from 6 back ("Hel"), then 0xC1 = space + 'A'
        assert_eq!(palmdoc_decompress(b"Hello \x80\x30\xc1"), b"Hello Hel A");
        // a run of raw bytes introduced by a count byte
        assert_eq!(palmdoc_decompress(&[3, 0xff, 0xfe, 0xfd]), [0xff, 0xfe, 0xfd]);
        // a copy that points before the start is ignored, not a crash
        assert_eq!(palmdoc_decompress(&[0x80, 0x08, b'x']), b"x");
    }

    #[test]
    fn trailing_data_is_measured() {
        // The end of a record, read backwards: first the entries of the flag bits 1.., each a size written with
        // 7 bits per byte (the byte with the top bit set is the last read), then, for bit 0, the multibyte overlap
        // (its last byte & 3, plus 1, is how many bytes it takes).
        let rec = [b'a', b'b', 0x99, 0x01, 0x81];
        assert_eq!(trailing_len(&rec, 0b11), 3, "a one-byte entry (0x81) and a two-byte overlap");
        assert_eq!(trailing_len(&[b'a', 0x81], 0b10), 1, "the entry counts its own size byte");
        assert_eq!(trailing_len(&[b'a', 0x01], 0b01), 2, "an overlap byte of 1 means 2 bytes");
        assert_eq!(trailing_len(b"abc", 0), 0);
    }

    #[test]
    fn a_protected_file_is_refused_not_decoded() {
        // a minimal Palm database whose first record says "encrypted"
        let mut d = vec![0u8; 78 + 16 + 40];
        d[60..68].copy_from_slice(b"BOOKMOBI");
        d[76..78].copy_from_slice(&2u16.to_be_bytes());
        d[78..82].copy_from_slice(&94u32.to_be_bytes());
        d[86..90].copy_from_slice(&120u32.to_be_bytes());
        d[94 + 12..94 + 14].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(read_mobi(&d).unwrap_err(), DRM_MESSAGE);
    }

    #[test]
    fn something_that_is_not_a_kindle_file_says_so() {
        assert!(read_mobi(b"hello").unwrap_err().contains("not a Kindle file"));
    }
}
