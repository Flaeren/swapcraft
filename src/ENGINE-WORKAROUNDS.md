# Where SwapCraft makes up for a gap in an ArtCraft engine

The converters stand on the official ArtCraft engines. Where an engine cannot do something, or does it wrongly, we
sometimes fix it on our side. **Rule (22/11/2026): every time an official engine is updated, check this list first. If
the new release fixes a gap below, remove our workaround, so we never carry two things for the same problem.**

For each engine that is behind, `python tools/check_engines.py` prints the entries below that belong to it. For each one:
read the new release's notes, run the check named in the entry against the bare new engine, and if the gap is closed,
take the workaround out (code, tests, and the note here) in the same change that aligns the engine.

Heading format: `## <Engine>: <what we fix>` (the script reads it).

## FilmCraft: old-codec AVI (Microsoft Video 1 / CRAM) is imported but not decoded
- **The gap:** FilmCraft opens such a file without a word and exports a "Media unreadable" card instead of the pictures.
- **Our fix:** `vf-core/src/msvideo1.rs` decodes the AVI into an uncompressed one that FilmCraft reads; `film.rs`
  (`undecodable_codec`, `decode_old_avi`) routes to it, and refuses with a clear message any other codec FilmCraft names
  only by its four letters.
- **Check:** probe an old CRAM AVI with the bare engine (`filmcraft-cli probe`): if it names the codec in words and gives a
  pixel format, then export one frame as PNG and look: a real picture, not the red card, means the gap is closed. The
  gated test `real_old_codec_avi_converts_to_mp4` (`VOIDFLARE_TEST_CRAM`) must still pass without our decoder.

## FilmCraft: a clip with no sound exported as sound only gives a silent file
- **Our fix:** `film.rs` refuses it (`film.no_sound`) by probing for audio streams first.
- **Check:** export a video with no audio as WAV/MP3 with the bare engine: if it now fails by itself, drop the probe.

## PhotoCraft: PNG to ICO above 256 pixels
- **The gap:** the engine refuses an image larger than 256x256 for ICO ("exceeds the format maximum of 256x256").
- **Our fix:** `job::shrink_to_icon` resizes with the engine (`image.imageSize`) and tries again; `engine_compat` tests it.
- **Check:** convert a 600x300 PNG to ICO with the bare engine.

## SoundCraft: writes 24-bit whatever it reads, and only WAV, FLAC, AIFF
- **Our fix:** `routes::audio_depth` picks the bit depth that keeps the source's resolution; MP3 and M4A go through
  FilmCraft (`audio.rs`).
- **Check:** convert a 16-bit WAV to WAV with the bare engine and read the bit depth back; look at the release notes for
  MP3/M4A output.

## WordCraft: documents embedded in a Word file come out as their icon
- **The gap:** the engine draws an OLE object as its icon and drops the document inside (reported as storytold/wordcraft#5393).
- **Our fix:** `vf-core/src/embedded.rs` reads `word/embeddings/` and appends the embedded documents to the PDF.
- **Check:** convert a DOCX with an embedded PDF/DOCX/XLSX with the bare engine and see whether the content is in the PDF.

## GridCraft: European-formatted CSV, leading zeros and long numbers
- **Our fix:** `vf-core/src/sheet.rs` rewrites `;` CSV numbers and dates written the European way (`1.234,5`,
  `05/11/2026`), and keeps `00123` and numbers over 15 digits as text, before handing the CSV to the engine.
- **Check:** feed such a CSV to the bare engine and compare the cells it reads with what the file says.

## CADCraft: tilted block inserts
- **The gap:** inserts of blocks with a tilt come out wrong (issue storytold/cadcraft#1158).
- **Our fix:** we build CADCraft ourselves with `vendor/cadcraft-tilted-insert.patch` (`vendor/cadcraft/`).
- **Check:** when the official release fixes it, drop `vendor/` and use the official engine (`engines.json`).

## Not a fix, a wish
- **PdfCraft PDF to Word/PowerPoint:** the layout is poor for graphic documents (coloured boxes lost, text split). We
  decided on 22/11/2026 to wait for ArtCraft rather than build our own rebuild; look at each new PdfCraft release for it.
  (If we ever build one: text from `text_lines`, pictures from `page_images`, boxes read with `lopdf`; never a PDF made
  of page images.)
