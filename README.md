<p align="center">
  <a href="https://voidflare.app">
    <img src="docs/img/logo.svg" alt="voidflare" width="360">
  </a>
</p>

<h3 align="center">Offline file converters for Windows. Drop a file, pick a format, done.</h3>

<p align="center">
  <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-6366f1.svg" alt="License: MIT OR Apache-2.0"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-1f2937.svg" alt="Platform: Windows 10 and 11">
  <img src="https://img.shields.io/badge/status-early%20(v0.1.0)-b45309.svg" alt="Status: early">
  <img src="https://img.shields.io/badge/made%20with-Rust-1f2937.svg" alt="Made with Rust">
  <img src="https://img.shields.io/badge/works-100%25%20offline-0f766e.svg" alt="Works 100% offline">
  <a href="CREDITS.md"><img src="https://img.shields.io/badge/built%20on-the%20ArtCraft%20suite-6366f1.svg" alt="Built on the ArtCraft suite"></a>
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#what-it-converts">What it converts</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#built-on-artcraft-credits-and-licences">Credits and licences</a> ·
  <a href="#build-from-source">Build</a> ·
  <a href="#contributing">Contributing</a> ·
  <a href="#licence">Licence</a>
</p>

<p align="center">
  <a href="https://voidflare.app"><img src="docs/img/hub.png" alt="The voidflare hub with a PDF loaded and the formats it can become" width="720"></a>
</p>

voidflare takes a file and offers the formats it can become. Your files never leave your PC, nothing needs to be installed, and there is no account. It is a small hub plus five apps; each app carries its own conversion engine, so you need no Microsoft Office and no extra downloads.

> **Status:** early (v0.1.0). The programs are not code-signed yet, so Windows SmartScreen may warn on first run (see Quick start).

## Built on ArtCraft: credits and licences

voidflare does not convert anything by itself. Every conversion is done by a command-line engine of the **[ArtCraft](https://getartcraft.com)** suite, written by its authors and used under their licences. This project is independent and not affiliated with or endorsed by the ArtCraft team.

| App | Engine | Engine licence |
|---|---|---|
| Word to PDF | WordCraft | MIT **or** Apache-2.0 |
| Excel to PDF | GridCraft | MIT **or** Apache-2.0 |
| Slides to PDF | DeckCraft | MIT **or** Apache-2.0 |
| PDF Tools | PdfCraft | MIT **or** Apache-2.0 |
| DWG to PDF | CADCraft, with one local fix ([patch](src/vendor/cadcraft-tilted-insert.patch)) | MIT **or** Apache-2.0 |

- The release package keeps each engine's `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE` and `ATTRIBUTION` files next to it. They list the further components, fonts and models each engine bundles.
- The ArtCraft name, logo and trademarks are not covered by those licences and are not used here as ours.
- Full details, libraries and translation status: **[CREDITS.md](CREDITS.md)** and **[NOTICE](NOTICE)**.
- voidflare's own code: **[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)**, at your option.

## Quick start

1. Download the ZIP from the [Releases](../../releases) page (about 210 MB, 430 MB once unzipped: each app carries its own engine).
2. Unzip it to a short path, for example `C:\voidflare`.
3. Run `voidflare.exe` and drop a file on it.

The programs are not code-signed yet, so Windows SmartScreen may warn on first run: choose *More info*, then *Run anyway*. Or unblock the ZIP (right click, Properties, *Unblock*) before extracting.

You can also drop a file on `voidflare.exe` in Explorer, or send it there with *Open with*. Each app also runs on its own from `apps\<app>\`.

## What it converts

| App | Reads | Writes |
|---|---|---|
| Word to PDF | DOCX, DOC, RTF, ODT | PDF |
| Excel to PDF | XLSX, CSV, TSV | PDF |
| Slides to PDF | PPTX | PDF |
| PDF Tools | PDF | DOCX, PPTX, HTML, RTF, TXT, PNG, JPG; also merge, split, extract pages, shrink, OCR (English only) |
| DWG to PDF | DWG | PDF: sheets merged or separate, or the whole drawing on one page |

Available in English, Italiano, Español, Français and Deutsch. Image, audio and video converters are planned.

## How it works

- **The hub** (`voidflare.exe`) has a drop panel. It looks at the file type, shows the formats that file can become, and runs the matching engine with default settings. The converted file lands next to the original.
- **The apps** have the full options: batches and folders, merging, page ranges, OCR, drawing sheets or the whole drawing.
- **The engines** are separate command-line programs that sit in each app's own folder. voidflare finds them next to the app first, then in a regular ArtCraft installation.
- **Nothing is sent anywhere.** Conversions run locally and there is no telemetry.
- **Updates.** When the hub starts it asks `github.com/Flaeren/voidflare` once for the latest release (one HTTPS request, no identifier, nothing about you). The *AUTO-CHECK* switch in the footer turns this off; the *CHECK FOR UPDATES* link does it on demand. If there is a newer version, a click downloads the ZIP into your Downloads folder and checks its SHA-256 against the release's `SHA256SUMS.txt`. voidflare never downloads without that click and never installs or replaces files by itself: unzip the new folder and run it.

## System requirements

- Windows 10 or 11, 64-bit.
- About 430 MB of disk space for the whole folder.
- No Visual C++ runtime, no Microsoft Office, no internet connection.

## Build from source

You need Windows, the Rust toolchain and an internet connection for the first build. The engines are not installed by hand: `fetch-engines.ps1` downloads the exact versions pinned in [`src/engines.json`](src/engines.json) from the official ArtCraft releases and refuses any file whose SHA-256 differs from the pinned one. CADCraft is built from source at a pinned commit with our patch.

```powershell
cd src
powershell -File fetch-engines.ps1 -BuildCadcraft   # engines, pinned and hash-checked (-BuildCadcraft needs git)
powershell -File build-apps.ps1                     # writes ..\portableoidflare.exe and ..\portableppspowershell -File release.ps1                        # packs portable\ into the ZIP and SHA256SUMS.txt
cargo test --release -p vf-core                     # tests that need an engine skip themselves when it is missing
```

```
src/
  hub/            the launcher with the drop panel
  crates/
    vf-core/      engine discovery, jobs, PDF and DWG operations, translations
    vf-ui/        theme and shared widgets
    app-*/        one crate per app
  vendor/         the patch for CADCraft
  licenses/       licence texts copied next to each bundled engine
  engines.json    the engines bundled, pinned by version and SHA-256
  fetch-engines.ps1  downloads and verifies them (and builds CADCraft with our patch)
  build-apps.ps1  builds everything into portable/
  release.ps1     packs portable/ into the release ZIP and SHA256SUMS.txt
site/             the website (voidflare.app)
docs/             images for this page
```

## Contributing

Bug reports, translation fixes and format requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Security issues: see [SECURITY.md](SECURITY.md).

## Licence

voidflare is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. The bundled engines have their own licences, shipped with them; see [Built on ArtCraft](#built-on-artcraft-credits-and-licences).

---

<p align="center"><sub>Made by Gabriele Denaro · <a href="https://github.com/Flaeren">GitHub: Flaeren</a> · <a href="https://gabrieledenaro.com">gabrieledenaro.com</a><br>Built on the <a href="https://getartcraft.com">ArtCraft</a> suite · Made with Rust</sub></p>
