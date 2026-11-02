# Credits and third-party notices

voidflare is a thin, friendly layer on top of other people's work. This file says whose.

## Author

voidflare is made by Gabriele Denaro: [GitHub: Flaeren](https://github.com/Flaeren) · [gabrieledenaro.com](https://gabrieledenaro.com).

## The conversion engines: the ArtCraft suite

Every conversion is done by a command-line engine from the [ArtCraft](https://getartcraft.com) suite. We did not write them and we are grateful to the people who did.

| voidflare app | Engine |
|---|---|
| Word to PDF | WordCraft |
| Excel to PDF | GridCraft |
| Slides to PDF | DeckCraft |
| PDF Tools | PdfCraft |
| DWG to PDF | CADCraft (a build with one local fix, see below) |
| Image Converter | PhotoCraft |
| Audio Converter | SoundCraft |
| Video Converter | FilmCraft |
| E-book Converter | WordCraft (PDF and Word); reading EPUB and Kindle books and writing EPUB is voidflare's own Rust code |

- Versions in voidflare 0.3.0: WordCraft 0.12.0, GridCraft 0.13.0, DeckCraft 0.12.0, PdfCraft 0.15.0, CADCraft 0.14.0, PhotoCraft 0.19.0, SoundCraft 0.16.0, FilmCraft 0.16.0. They are pinned, with their SHA-256, in [`src/engines.json`](src/engines.json).
- The engines are distributed under the terms **MIT License or Apache License 2.0, at your option**, as stated in the `NOTICE` file shipped with each engine.
- When an app brings its own engine (always for DWG to PDF, and for the others unless your PC already has a tested ArtCraft one, in which case nothing of it is copied and its own licence files stay in its installation), the pack carries it unmodified (apart from CADCraft) in the app's own folder, together with its `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE` and `ATTRIBUTION` files. Those files list the further components, fonts and models each engine bundles, with their own licences. **Keep them next to the engine if you redistribute it.**
- The ArtCraft name, logo and trademarks are **not** covered by those licences and are not used by voidflare as its own. voidflare is an independent project and is not affiliated with or endorsed by the ArtCraft team. Where we mention ArtCraft we do it in plain text and link to their site.

### CADCraft and the local fix

The DWG to PDF app uses a CADCraft build that contains a small fix for blocks inserted with a tilted extrusion direction (they were drawn as a ramp instead of flat). The change is published as `src/vendor/cadcraft-tilted-insert.patch`. The same approach (map the insertion point to the world coordinates and keep the block flat) is proposed in the upstream issue [storytold/cadcraft#1158](https://github.com/storytold/cadcraft/issues/1158), which was reported by someone else; we checked our build against the test file from that issue. We intend to drop the patch as soon as the upstream project fixes the problem. The patched build is not stored in this repository.

## Libraries we build on

voidflare is written in Rust. The interface uses [egui](https://github.com/emilk/egui) and `eframe` (MIT OR Apache-2.0), through the forks the ArtCraft launchers use, and the other crates listed in `src/Cargo.lock` under their own licences (MIT, Apache-2.0 or similar). PDF merging uses [lopdf](https://github.com/J-F-Liu/lopdf).

## Fonts and artwork

Fonts are credited in each engine's `ATTRIBUTION.md` and in the licences of the Rust crates that carry them. The wordmark "voidflare" in the logo and on the website is set in **Michroma**, copyright 2011 The Michroma Project Authors ([source](https://github.com/googlefonts/Michroma-font)), under the [SIL Open Font License 1.1](https://scripts.sil.org/OFL); the logo uses it as outlines (`src/tools/make_logo.py`). The text of the website is set in **Instrument Sans** (copyright 2022 The Instrument Sans Project Authors, [source](https://github.com/Instrument/instrument-sans)) and **JetBrains Mono** (copyright 2020 The JetBrains Mono Project Authors, [source](https://github.com/JetBrains/JetBrainsMono)), both under the SIL Open Font License 1.1. The website serves its own copy of the three fonts (latin subset, `site/public/fonts/`, with their OFL texts) and requests nothing from Google. The flame mark and the app icons were made for this project.

## Translations

English is the source language. The Italian, Spanish, French and German texts were written with machine help and **have not yet been reviewed by native speakers**. Corrections are very welcome.

## Licence of voidflare itself

voidflare's own code is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
