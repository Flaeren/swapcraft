//! Old AVI videos made with Microsoft Video 1 (fourcc `CRAM`, `MSVC`, `WHAM`): the codec of the first screen recorders
//! and of many video clips from the 1990s and early 2000s. FilmCraft cannot decode it (it imports such a file without
//! a word and then exports a "Media unreadable" card), so SwapCraft does it here: read the AVI, decode every frame, and
//! write the pictures (and the sound, when it is plain PCM) as an uncompressed AVI that FilmCraft reads. The result then
//! goes through the same export as any other video.
//!
//! The codec works on 4x4 blocks, bottom row of the picture first. A block is skipped (it keeps what the previous
//! frame had), one colour, two colours (a 16-bit mask picks one of the two for each pixel) or eight colours (the same,
//! with a pair of colours for each 2x2 quarter). It has two flavours: 8 bits per pixel with a palette, and 15 bits
//! (RGB 5-5-5). Written from the published description of the format, with no code taken from another decoder.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// The four-character codes of Microsoft Video 1.
pub fn is_msvideo1(fourcc: &[u8]) -> bool {
    let f = fourcc.to_ascii_uppercase();
    f == b"CRAM" || f == b"MSVC" || f == b"WHAM"
}

/// What a probe of a file says about its video codec, to decide whether this module is the way to read it. FilmCraft's
/// probe names it `CRAM` (the name of the code, as it found it in the AVI).
pub fn is_msvideo1_name(name: &str) -> bool {
    is_msvideo1(name.trim().as_bytes())
}

// ---------------------------------------------------------------------------------------------- reading the AVI

/// The video and (when there is one) the sound of an AVI, and where the chunks of the `movi` list are.
struct Avi {
    width: usize,
    height: usize,
    bits: u16,
    compression: [u8; 4],
    scale: u32,
    rate: u32,
    /// The colour table of a palettised picture: red, green, blue.
    palette: Vec<[u8; 3]>,
    /// The sound stream's own header and format, copied as they are when it is plain PCM.
    audio: Option<Audio>,
    /// The index of the video stream and of the audio one, to tell their chunks (`00dc`, `01wb`) apart.
    video_index: u8,
    /// Offset and length of each chunk of the `movi` list, in the order they appear.
    chunks: Vec<Chunk>,
}

struct Audio {
    index: u8,
    strh: Vec<u8>,
    strf: Vec<u8>,
}

struct Chunk {
    id: [u8; 4],
    start: usize,
    len: usize,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// The stream number in a chunk id: `00dc` is stream 0, `01wb` stream 1.
fn stream_of(id: &[u8; 4]) -> Option<u8> {
    let (a, b) = (id[0], id[1]);
    (a.is_ascii_digit() && b.is_ascii_digit()).then(|| (a - b'0') * 10 + (b - b'0'))
}

fn parse(bytes: &[u8]) -> Result<Avi, String> {
    let bad = || "this is not an AVI file this program can read".to_owned();
    if bytes.get(0..4) != Some(b"RIFF") || !matches!(bytes.get(8..12), Some(b"AVI ") | Some(b"AVIX")) {
        return Err(bad());
    }
    let mut avi = Avi { width: 0, height: 0, bits: 0, compression: [0; 4], scale: 1, rate: 25, palette: Vec::new(), audio: None, video_index: 0, chunks: Vec::new() };
    let mut have_video = false;
    // the strh of the stream being read: its type, and the whole header
    let mut cur: Option<([u8; 4], Vec<u8>)> = None;
    let mut stream_no: i32 = -1;
    let mut stack: Vec<usize> = vec![bytes.len()]; // end offset of each list being read
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        while stack.len() > 1 && pos >= *stack.last().unwrap() {
            stack.pop();
        }
        let id: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
        let size = u32_at(bytes, pos + 4).ok_or_else(bad)? as usize;
        let body = pos + 8;
        let end = body.checked_add(size).ok_or_else(bad)?.min(bytes.len());
        match &id {
            b"LIST" | b"RIFF" => {
                let kind: [u8; 4] = bytes.get(body..body + 4).ok_or_else(bad)?.try_into().unwrap();
                if &kind == b"strl" {
                    stream_no += 1;
                    cur = None;
                }
                if &kind == b"movi" {
                    // the pictures and the sound, one chunk after the other (a `rec ` list inside is walked into)
                    stack.push(end);
                    pos = body + 4;
                    continue;
                }
                stack.push(end);
                pos = body + 4;
                continue;
            }
            b"strh" => {
                let h = bytes.get(body..end).ok_or_else(bad)?.to_vec();
                let kind: [u8; 4] = h.get(0..4).ok_or_else(bad)?.try_into().unwrap();
                cur = Some((kind, h));
            }
            b"strf" => {
                if let Some((kind, strh)) = &cur {
                    let f = bytes.get(body..end).ok_or_else(bad)?;
                    if kind == b"vids" && !have_video {
                        have_video = true;
                        avi.video_index = stream_no.max(0) as u8;
                        avi.width = u32_at(f, 4).ok_or_else(bad)? as usize;
                        avi.height = (u32_at(f, 8).ok_or_else(bad)? as i32).unsigned_abs() as usize;
                        avi.bits = u16_at(f, 14).ok_or_else(bad)?;
                        avi.compression = f.get(16..20).ok_or_else(bad)?.try_into().unwrap();
                        avi.scale = u32_at(strh, 20).unwrap_or(1).max(1);
                        avi.rate = u32_at(strh, 24).unwrap_or(25).max(1);
                        let used = u32_at(f, 32).unwrap_or(0) as usize;
                        let colours = if used == 0 { 1usize << avi.bits.min(8) } else { used };
                        if avi.bits <= 8 {
                            for i in 0..colours.min(256) {
                                let o = 40 + i * 4;
                                if let Some(e) = f.get(o..o + 4) {
                                    avi.palette.push([e[2], e[1], e[0]]);
                                }
                            }
                        }
                    } else if kind == b"auds" && avi.audio.is_none() {
                        // plain PCM only: WAVEFORMATEX.wFormatTag == 1
                        if u16_at(f, 0) == Some(1) {
                            avi.audio = Some(Audio { index: stream_no.max(0) as u8, strh: strh.clone(), strf: f.to_vec() });
                        }
                    }
                }
            }
            _ => {
                if stream_of(&id).is_some() && stack.len() > 1 {
                    avi.chunks.push(Chunk { id, start: body, len: end.saturating_sub(body) });
                }
            }
        }
        pos = body + size + (size & 1);
    }
    if !have_video || avi.width == 0 || avi.height == 0 {
        return Err("this AVI has no video stream".to_owned());
    }
    Ok(avi)
}

// ---------------------------------------------------------------------------------------------- the decoder

/// The picture being rebuilt, frame after frame (a skipped block keeps what the last frame had there).
struct Decoder {
    width: usize,
    height: usize,
    bits: u16,
    /// Top-down, one entry per pixel: a palette index (8 bits) or an RGB 5-5-5 word (16 bits).
    pix: Vec<u16>,
    palette: Vec<[u8; 3]>,
}

impl Decoder {
    fn new(avi: &Avi) -> Self {
        let mut palette = avi.palette.clone();
        palette.resize(256, [0, 0, 0]);
        Decoder { width: avi.width, height: avi.height, bits: avi.bits, pix: vec![0; avi.width * avi.height], palette }
    }

    /// A palette change (`00pc`): `first` entry, then `n` entries of (red, green, blue, flags). 0 entries means 256.
    fn change_palette(&mut self, data: &[u8]) {
        let (first, n) = (*data.first().unwrap_or(&0) as usize, *data.get(1).unwrap_or(&0) as usize);
        let n = if n == 0 { 256 } else { n };
        for i in 0..n {
            if let Some(e) = data.get(4 + i * 4..8 + i * 4) {
                if let Some(slot) = self.palette.get_mut(first + i) {
                    *slot = [e[0], e[1], e[2]];
                }
            }
        }
    }

    fn put(&mut self, bx: usize, by: usize, px: usize, py: usize, value: u16) {
        // `py` counts rows up from the bottom row of the block, as the format does
        let (x, y) = (bx * 4 + px, by * 4 + 3 - py);
        if x < self.width && y < self.height {
            self.pix[y * self.width + x] = value;
        }
    }

    /// Decodes one frame into the picture. A damaged or short frame stops where it is: the rest keeps the old picture.
    fn frame(&mut self, data: &[u8]) {
        let (blocks_wide, blocks_high) = (self.width.div_ceil(4), self.height.div_ceil(4));
        let mut total = blocks_wide * blocks_high;
        let mut skip = 0usize;
        let mut p = 0usize;
        let word = self.bits >= 15;
        let take = |p: &mut usize, n: usize| -> Option<usize> {
            let at = *p;
            if at + n > data.len() {
                return None;
            }
            *p += n;
            Some(at)
        };
        // blocks run left to right, the bottom row of blocks first
        for by in (0..blocks_high).rev() {
            for bx in 0..blocks_wide {
                if skip > 0 {
                    skip -= 1;
                    total = total.saturating_sub(1);
                    continue;
                }
                let Some(at) = take(&mut p, 2) else { return };
                let (a, b) = (data[at], data[at + 1]);
                if a == 0 && b == 0 && total == 0 {
                    return;
                }
                if (b & 0xFC) == 0x84 {
                    // skip: this block and the next ones keep their old pixels (the count includes this block)
                    skip = (((b as usize) - 0x84) << 8) + a as usize;
                    skip = skip.saturating_sub(1);
                    total = total.saturating_sub(1);
                    continue;
                }
                let flags = ((b as u32) << 8) | a as u32;
                if word {
                    if b < 0x80 {
                        let Some(at) = take(&mut p, 4) else { return };
                        let c0 = u16::from_le_bytes([data[at], data[at + 1]]);
                        let c1 = u16::from_le_bytes([data[at + 2], data[at + 3]]);
                        if c0 & 0x8000 != 0 {
                            // eight colours: a pair for each 2x2 quarter
                            let Some(at) = take(&mut p, 12) else { return };
                            let mut c = [c0, c1, 0, 0, 0, 0, 0, 0];
                            for (i, slot) in c.iter_mut().enumerate().skip(2) {
                                *slot = u16::from_le_bytes([data[at + (i - 2) * 2], data[at + (i - 2) * 2 + 1]]);
                            }
                            let mut f = flags;
                            for py in 0..4 {
                                for px in 0..4 {
                                    self.put(bx, by, px, py, c[((py & 2) << 1) + (px & 2) + ((f & 1) ^ 1) as usize]);
                                    f >>= 1;
                                }
                            }
                        } else {
                            let mut f = flags;
                            for py in 0..4 {
                                for px in 0..4 {
                                    self.put(bx, by, px, py, if f & 1 != 0 { c0 } else { c1 });
                                    f >>= 1;
                                }
                            }
                        }
                    } else {
                        // one colour: the two bytes already read are the colour
                        let c = flags as u16;
                        for py in 0..4 {
                            for px in 0..4 {
                                self.put(bx, by, px, py, c);
                            }
                        }
                    }
                } else if b < 0x80 {
                    let Some(at) = take(&mut p, 2) else { return };
                    let (c0, c1) = (data[at] as u16, data[at + 1] as u16);
                    let mut f = flags;
                    for py in 0..4 {
                        for px in 0..4 {
                            self.put(bx, by, px, py, if f & 1 != 0 { c0 } else { c1 });
                            f >>= 1;
                        }
                    }
                } else if b >= 0x90 {
                    let Some(at) = take(&mut p, 8) else { return };
                    let mut f = flags;
                    for py in 0..4 {
                        for px in 0..4 {
                            self.put(bx, by, px, py, data[at + ((py & 2) << 1) + (px & 2) + ((f & 1) ^ 1) as usize] as u16);
                            f >>= 1;
                        }
                    }
                } else {
                    // one colour: the first byte is the palette index
                    for py in 0..4 {
                        for px in 0..4 {
                            self.put(bx, by, px, py, a as u16);
                        }
                    }
                }
                total = total.saturating_sub(1);
            }
        }
    }

    /// The picture as 24-bit rows, bottom row first, each padded to a multiple of four bytes (blue, green, red).
    fn dib(&self, out: &mut Vec<u8>) {
        out.clear();
        let pad = (4 - (self.width * 3) % 4) % 4;
        for y in (0..self.height).rev() {
            for x in 0..self.width {
                let v = self.pix[y * self.width + x];
                let [r, g, b] = if self.bits >= 15 {
                    let five = |s: u16| {
                        let c = ((v >> s) & 31) as u32;
                        ((c << 3) | (c >> 2)) as u8
                    };
                    [five(10), five(5), five(0)]
                } else {
                    self.palette[(v & 0xFF) as usize]
                };
                out.extend_from_slice(&[b, g, r]);
            }
            out.extend(std::iter::repeat_n(0u8, pad));
        }
    }
}

// ---------------------------------------------------------------------------------------------- writing the result

/// Appends a chunk; returns where its body starts.
fn put_chunk(w: &mut BufWriter<File>, id: &[u8; 4], body: &[u8]) -> std::io::Result<()> {
    w.write_all(id)?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(body)?;
    if body.len() % 2 == 1 {
        w.write_all(&[0])?;
    }
    Ok(())
}

fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// What the decoding made, for the caller and for tests.
#[derive(Debug, PartialEq)]
pub struct Made {
    pub frames: usize,
    pub width: usize,
    pub height: usize,
    /// The sound was carried over (plain PCM); false when there was none or it was in a format that is left out.
    pub audio: bool,
}

/// How many bytes an uncompressed copy of the picture would take, to refuse a video that would fill the disk.
pub fn raw_size(avi_bytes: &[u8]) -> Option<u64> {
    let avi = parse(avi_bytes).ok()?;
    let frames = avi.chunks.iter().filter(|c| stream_of(&c.id) == Some(avi.video_index) && matches!(&c.id[2..], b"dc" | b"db")).count() as u64;
    Some(frames * ((avi.width * 3 + 3) / 4 * 4 * avi.height) as u64)
}

/// Decodes the Microsoft Video 1 AVI at `src` into an uncompressed AVI at `dst`.
pub fn to_raw_avi(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<Made, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let avi = parse(&bytes)?;
    if !is_msvideo1(&avi.compression) {
        return Err(format!("this video is not Microsoft Video 1 (its codec is {})", String::from_utf8_lossy(&avi.compression).trim()));
    }
    if avi.bits != 8 && avi.bits != 15 && avi.bits != 16 {
        return Err(format!("Microsoft Video 1 at {} bits per pixel is not supported", avi.bits));
    }
    let mut dec = Decoder::new(&avi);
    let (w, h) = (avi.width, avi.height);
    let row = (w * 3).div_ceil(4) * 4;
    let image_size = row * h;
    let out = File::create(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut o = BufWriter::new(out);
    let io = |e: std::io::Error| e.to_string();

    // the headers, with the sizes and counts to be written again at the end
    o.write_all(b"RIFF\0\0\0\0AVI ").map_err(io)?;
    o.write_all(b"LIST\0\0\0\0hdrl").map_err(io)?;
    let micros = ((1_000_000u64 * avi.scale as u64) / avi.rate as u64) as u32;
    let streams = 1 + avi.audio.is_some() as u32;
    let mut avih = Vec::new();
    for v in [micros, 0, 0, 0x10 /* has an index */, 0 /* frames: patched */, 0, streams, image_size as u32, w as u32, h as u32, 0, 0, 0, 0] {
        avih.extend_from_slice(&le32(v));
    }
    put_chunk(&mut o, b"avih", &avih).map_err(io)?;
    // video stream header and format
    let mut strh = Vec::new();
    strh.extend_from_slice(b"vidsDIB ");
    for v in [0u32, 0, 0, avi.scale, avi.rate, 0, 0 /* length: patched */, image_size as u32, 0xFFFF_FFFF, 0] {
        strh.extend_from_slice(&le32(v));
    }
    strh.extend_from_slice(&[0, 0, 0, 0, w as u8, (w >> 8) as u8, h as u8, (h >> 8) as u8]);
    let mut strf = Vec::new();
    for v in [40u32, w as u32, h as u32] {
        strf.extend_from_slice(&le32(v));
    }
    strf.extend_from_slice(&1u16.to_le_bytes());
    strf.extend_from_slice(&24u16.to_le_bytes());
    for v in [0u32 /* BI_RGB */, image_size as u32, 0, 0, 0, 0] {
        strf.extend_from_slice(&le32(v));
    }
    let strl_pos = o.stream_position().map_err(io)?;
    o.write_all(b"LIST\0\0\0\0strl").map_err(io)?;
    put_chunk(&mut o, b"strh", &strh).map_err(io)?;
    put_chunk(&mut o, b"strf", &strf).map_err(io)?;
    let strl_end = o.stream_position().map_err(io)?;
    o.seek(SeekFrom::Start(strl_pos + 4)).map_err(io)?;
    o.write_all(&le32((strl_end - strl_pos - 8) as u32)).map_err(io)?;
    o.seek(SeekFrom::Start(strl_end)).map_err(io)?;
    let mut audio_strh_len_at = None;
    if let Some(a) = &avi.audio {
        let pos = o.stream_position().map_err(io)?;
        o.write_all(b"LIST\0\0\0\0strl").map_err(io)?;
        let mut sh = a.strh.clone();
        sh.resize(56, 0);
        audio_strh_len_at = Some(o.stream_position().map_err(io)? + 8 + 32);
        put_chunk(&mut o, b"strh", &sh).map_err(io)?;
        put_chunk(&mut o, b"strf", &a.strf).map_err(io)?;
        let end = o.stream_position().map_err(io)?;
        o.seek(SeekFrom::Start(pos + 4)).map_err(io)?;
        o.write_all(&le32((end - pos - 8) as u32)).map_err(io)?;
        o.seek(SeekFrom::Start(end)).map_err(io)?;
    }
    let hdrl_end = o.stream_position().map_err(io)?;
    o.seek(SeekFrom::Start(16)).map_err(io)?;
    o.write_all(&le32((hdrl_end - 20) as u32)).map_err(io)?;
    o.seek(SeekFrom::Start(hdrl_end)).map_err(io)?;

    // the pictures (and the sound), in the order they came
    let movi_pos = o.stream_position().map_err(io)?;
    o.write_all(b"LIST\0\0\0\0movi").map_err(io)?;
    let mut index: Vec<([u8; 4], u32, u32)> = Vec::new();
    let mut frames = 0usize;
    let mut audio_bytes = 0u32;
    let mut dib = Vec::with_capacity(image_size);
    for c in &avi.chunks {
        if cancelled() {
            return Err(crate::job::CANCELLED.to_owned());
        }
        let data = &bytes[c.start..c.start + c.len];
        let stream = stream_of(&c.id).unwrap_or(255);
        let kind = &c.id[2..];
        if stream == avi.video_index {
            match kind {
                b"pc" => dec.change_palette(data),
                b"dc" | b"db" => {
                    if !data.is_empty() {
                        dec.frame(data);
                    }
                    dec.dib(&mut dib);
                    let at = o.stream_position().map_err(io)? - movi_pos - 8;
                    index.push((*b"00db", at as u32, dib.len() as u32));
                    put_chunk(&mut o, b"00db", &dib).map_err(io)?;
                    frames += 1;
                }
                _ => {}
            }
        } else if let Some(a) = &avi.audio {
            if stream == a.index && kind == b"wb" {
                let id = [b'0', b'1', b'w', b'b'];
                let at = o.stream_position().map_err(io)? - movi_pos - 8;
                index.push((id, at as u32, data.len() as u32));
                audio_bytes += data.len() as u32;
                put_chunk(&mut o, &id, data).map_err(io)?;
            }
        }
    }
    if frames == 0 {
        return Err("the video has no pictures".to_owned());
    }
    let movi_end = o.stream_position().map_err(io)?;
    // the index
    let mut idx = Vec::with_capacity(index.len() * 16);
    for (id, at, len) in &index {
        idx.extend_from_slice(id);
        idx.extend_from_slice(&le32(0x10));
        idx.extend_from_slice(&le32(*at));
        idx.extend_from_slice(&le32(*len));
    }
    put_chunk(&mut o, b"idx1", &idx).map_err(io)?;
    let end = o.stream_position().map_err(io)?;
    // sizes and counts known only now
    let patch = |o: &mut BufWriter<File>, at: u64, v: u32| -> Result<(), String> {
        o.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        o.write_all(&le32(v)).map_err(|e| e.to_string())
    };
    patch(&mut o, 4, (end - 8) as u32)?;
    patch(&mut o, movi_pos + 4, (movi_end - movi_pos - 8) as u32)?;
    // avih.dwTotalFrames: the avih body starts 8 bytes in, at 24 + 8 + 16
    patch(&mut o, 24 + 8 + 16, frames as u32)?;
    // strh.dwLength of the video stream: its body starts 8 bytes in, 32 bytes of the header precede dwLength
    patch(&mut o, strl_pos + 12 + 8 + 32, frames as u32)?;
    if let Some(at) = audio_strh_len_at {
        // the sound stream keeps the length its source gave; only a stream with no length is filled in
        let _ = (at, audio_bytes);
    }
    o.flush().map_err(io)?;
    Ok(Made { frames, width: w, height: h, audio: avi.audio.is_some() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-block-wide strip 4x4 pixels at 15 bits, with one block of each kind, as raw frame bytes.
    fn decoder(bits: u16, w: usize, h: usize) -> Decoder {
        Decoder { width: w, height: h, bits, pix: vec![0; w * h], palette: (0..256).map(|i| [i as u8, 0, 0]).collect() }
    }

    fn word(v: u16) -> [u8; 2] {
        v.to_le_bytes()
    }

    #[test]
    fn a_one_colour_block_fills_its_four_by_four_pixels() {
        let mut d = decoder(16, 4, 4);
        // a one-colour block is the colour itself with the top bit set (that is what tells it from the other kinds)
        d.frame(&word(0x8000 | 0x7C00)); // red
        assert!(d.pix.iter().all(|&p| p == 0xFC00), "{:?}", d.pix);
        let mut out = Vec::new();
        d.dib(&mut out);
        assert_eq!(&out[0..3], &[0, 0, 255], "the top bit is not part of the colour: pure red");
    }

    #[test]
    fn a_two_colour_block_takes_the_mask_bottom_row_first() {
        let mut d = decoder(16, 4, 4);
        // mask 0b0000_0000_0000_0011: the first two pixels (bottom row, left) use colour 0, the rest colour 1
        let mut f = vec![0x03, 0x00];
        f.extend_from_slice(&word(0x001F)); // colour 0: blue
        f.extend_from_slice(&word(0x03E0)); // colour 1: green
        d.frame(&f);
        assert_eq!(d.pix[3 * 4], 0x001F, "bottom row, first pixel");
        assert_eq!(d.pix[3 * 4 + 1], 0x001F, "bottom row, second pixel");
        assert_eq!(d.pix[3 * 4 + 2], 0x03E0, "bottom row, third pixel");
        assert_eq!(d.pix[0], 0x03E0, "the top row is the last one written");
    }

    #[test]
    fn an_eight_colour_block_uses_a_pair_for_each_quarter() {
        let mut d = decoder(16, 4, 4);
        // mask all ones: every pixel takes the first colour of its quarter's pair
        let mut f = vec![0xFF, 0xFF & 0x7F];
        f.extend_from_slice(&word(0x8000 | 0x0001)); // colour 0 has the top bit set: eight-colour mode
        for c in 2..=8u16 {
            f.extend_from_slice(&word(c)); // colours 1..7 are 2..=8
        }
        // 2 + 2 + 12 bytes were needed: colour 0, colour 1 and six more
        f.truncate(2 + 4 + 12);
        d.frame(&f);
        // the bottom-left quarter (rows 2-3, columns 0-1) uses colours 0/1, the bottom-right 2/3, the top-left 4/5
        assert_eq!(d.pix[3 * 4] & 0x7FFF, 0x0001 & 0x7FFF, "bottom-left quarter");
        assert_ne!(d.pix[3 * 4 + 2], d.pix[3 * 4], "the bottom-right quarter has its own pair");
        assert_ne!(d.pix[0], d.pix[3 * 4], "so has the top-left one");
    }

    #[test]
    fn a_skipped_block_keeps_the_picture_the_last_frame_had() {
        let mut d = decoder(16, 8, 4);
        // frame 1: two blocks of one colour each (the left block is decoded first)
        let mut f = Vec::new();
        f.extend_from_slice(&word(0x8000 | 0x7C00));
        f.extend_from_slice(&word(0x8000 | 0x03E0));
        d.frame(&f);
        let before = d.pix.clone();
        // frame 2: skip both blocks (code 0x8402: two blocks)
        d.frame(&[0x02, 0x84]);
        assert_eq!(d.pix, before);
        // frame 3: skip the first, change the second
        let mut f = vec![0x01, 0x84];
        f.extend_from_slice(&word(0x8000 | 0x001F));
        d.frame(&f);
        assert_eq!(d.pix[0], 0xFC00, "the left block was skipped");
        assert_eq!(d.pix[7], 0x801F, "the right block changed");
    }

    #[test]
    fn eight_bit_blocks_use_the_palette() {
        let mut d = decoder(8, 4, 4);
        d.frame(&[5, 0x00]); // one colour: a = 5 is the palette index (b = 0 would be two colours, so use 0x80)
        let mut d = decoder(8, 4, 4);
        d.frame(&[5, 0x80]);
        assert!(d.pix.iter().all(|&p| p == 5));
        let mut out = Vec::new();
        d.dib(&mut out);
        assert_eq!(&out[0..3], &[0, 0, 5], "blue, green, red of palette entry 5 = (5,0,0) is red 5");
    }

    #[test]
    fn a_short_frame_stops_without_a_panic() {
        let mut d = decoder(16, 8, 8);
        d.frame(&[0x03]); // not even one block header
        d.frame(&[0x03, 0x00, 0x1F]); // a two-colour block that is cut short
        assert!(d.pix.iter().all(|&p| p == 0));
    }

    #[test]
    fn the_picture_comes_out_as_bottom_up_bgr_rows() {
        let mut d = decoder(16, 4, 4);
        d.pix[0] = 0x7C00; // top-left: red
        d.pix[15] = 0x001F; // bottom-right: blue
        let mut out = Vec::new();
        d.dib(&mut out);
        assert_eq!(out.len(), 4 * 4 * 3);
        // bottom row first: its last pixel is blue (B=255, G=0, R=0)
        assert_eq!(&out[9..12], &[255, 0, 0]);
        // the top row is last: its first pixel is red (B=0, G=0, R=255)
        assert_eq!(&out[36..39], &[0, 0, 255]);
    }

    #[test]
    fn the_fourcc_names_are_recognised() {
        for n in ["CRAM", "cram", "MSVC", "WHAM"] {
            assert!(is_msvideo1_name(n), "{n}");
        }
        assert!(!is_msvideo1_name("H.264 (High)"));
    }

    /// With `VOIDFLARE_TEST_CRAM` (an AVI made with Microsoft Video 1): the whole file decodes, every frame, and the
    /// result is an AVI of its own that reads back with the same size and frame count.
    #[test]
    fn real_cram_file_decodes_to_a_raw_avi() {
        let Some(path) = std::env::var_os("VOIDFLARE_TEST_CRAM") else { return };
        let dir = crate::job::scratch_dir().unwrap();
        let out = dir.join("raw.avi");
        let made = to_raw_avi(Path::new(&path), &out, &|| false).expect("decodes");
        assert!(made.frames > 0 && made.width > 0 && made.height > 0, "{made:?}");
        let again = parse(&std::fs::read(&out).unwrap()).expect("the result is an AVI");
        let frames = again.chunks.iter().filter(|c| &c.id == b"00db").count();
        assert_eq!((again.width, again.height, again.bits, frames), (made.width, made.height, 24, made.frames));
        assert_eq!(&again.compression, &[0, 0, 0, 0], "uncompressed");
        // to look at the result by hand: VOIDFLARE_TEST_CRAM_OUT=<file> keeps a copy
        if let Some(keep) = std::env::var_os("VOIDFLARE_TEST_CRAM_OUT") {
            std::fs::copy(&out, keep).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
