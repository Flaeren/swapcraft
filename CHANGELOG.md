# Changelog

All notable changes to voidflare are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.3.4] - 2026-11-03

### Fixed

- **Word to PDF no longer loses the documents embedded in a Word file.** A PDF, Word, Excel or PowerPoint file inserted with *Insert > Object* (typically inside a table) was drawn as its small preview picture and the document inside was lost. Their pages are now read out of the file and added at the end of the PDF, one bookmark each, and the log says how many. Objects that cannot be read (old binary Office files, linked files) stay as a picture and are counted in the log.
- A relative file name on the command line (`voidflare-cli convert file.docx --to pdf`) failed, because the engine looked for it in its scratch folder.

## [0.3.3] - 2026-11-03

### Added

- **Choose the appearance in the settings:** *Same as Windows* (as before, and still the default), *Dark* or *Light*. The hub changes at once; the converter apps use the same choice the next time they start.
- **Drag and drop now shows itself.** While files are dragged over an app, a veil with a border and "Drop the files here to add them" says where to let go; files that cannot be added (a type the app does not read, or already in the list) are now reported instead of silently ignored. Applies to every converter app, PDF Tools and DWG to PDF included.

### Changed

- The messages about the Start menu shortcut now say what it does: voidflare is added to the *list of all apps*, and pinning it to the Start menu (or the taskbar) is a step Windows leaves to you, with the exact clicks written in the welcome dialog, after adding, and in the settings.

## [0.3.2] - 2026-11-02

### Added

- **UPDATE ALL** in the hub's toolbar (shown when two or more installed apps have an update): updates them one after the other; a failed or cancelled update stops the rest.

### Fixed

- **The Start menu shortcut could be reported as added when it was not there.** The hub now asks Windows for the real Start menu folder (it can be redirected on office PCs) instead of assuming one, shows where the shortcut was put, looks again two seconds later and says so if it has been removed (by security software or a policy), and has a *Show it* button that opens the folder with the shortcut selected.

### Changed

- voidflare now has a tagline: *Craft your conversion, always offline.* It is on the website, at the top of the README and in the hub's header.

## [0.3.1] - 2026-11-02

### Added

- **A welcome dialog on the first run** asks whether to add voidflare to the Start menu (it is asked once, and never again whatever you answer). Windows does not let a program pin itself to the taskbar, so the dialog says how to do that last step by hand: right-click the icon in the taskbar and choose *Pin to taskbar*.
- **A settings window**, behind the gear in the top right corner of the hub: the language of the converter apps, the update check, the Start menu shortcut (add, remove, or repoint it if the folder was moved) and a link to the folder where the few settings are kept. Installed apps are still removed from their own card (*Remove*).

### Changed

- **English is the default language.** The apps no longer follow the language of Windows: they speak English until you choose another one in the settings, and then every app uses that choice. (PCs that relied on the Windows language switch back to English once; pick it again in the settings.)

## [0.3.0] - 2026-11-02

### Added

- **A command line and an MCP server for AI assistants.** `voidflare-cli.exe` sits beside the hub: `voidflare-cli formats` lists what converts to what, `voidflare-cli convert <file> --to <format>` converts one file (`--json` for scripts), `voidflare-cli engines` says which engines were found. `voidflare-cli mcp` is a Model Context Protocol server on stdin/stdout, so an assistant such as Claude can convert files, merge, split, extract, shrink and OCR PDFs, and read file information, with the same engines and the same results as the apps. It may only touch the folder it was started in plus each `--allow <folder>` you name (`--allow-all` lifts that, and has to be asked for by name); paths are resolved first, so `..` and links cannot lead out. It updates together with the hub.
- **Image Converter** (PhotoCraft): PNG, JPG, WebP, TIFF, BMP, GIF, ICO, TGA, PSD and more in; PNG, JPG (with a quality slider), WebP, TIFF, BMP, GIF, ICO, TGA and PSD out; one image or a folder.
- **Audio Converter** (SoundCraft): MP3, OGG, M4A, AAC, FLAC, WAV, AIFF and more in; WAV, FLAC or AIFF out, at 16-bit, 24-bit or 32-bit float, with 44.1 kHz resampling and volume normalising. SoundCraft cannot write MP3 or OGG.
- **Video Converter** (FilmCraft): MP4, MOV, MKV, WebM, AVI, MPEG, MXF and more in; MP4 (H.264, good quality or small file), ProRes MOV, animated GIF, or just the sound as WAV, AAC (M4A) or FLAC; *half size* and *remove the sound* options. A clip without sound says so instead of writing a silent file. H.265 is not offered: FilmCraft writes it only through an NVIDIA hardware encoder.
- **E-book Converter**, written in Rust: EPUB and Kindle books (AZW3, AZW, MOBI, PRC) to EPUB, text, HTML (one page, pictures inside), PDF or Word; text, HTML, Markdown and Word files to EPUB. Books **with DRM are refused with a clear message**; nothing here removes copy protection. WordCraft is used only for PDF and Word. Checked on a real book published in both formats: the text read from the AZW3 and from the EPUB differs by one word in 33,441.
- The hub's drop panel offers the new conversions for the new file types.

### Changed

- **The hub now really upgrades itself.** *UPDATE NOW* downloads the new hub, checks its SHA-256, replaces the running `voidflare.exe` (and `voidflare-cli.exe` beside it) and restarts it, instead of only saving a ZIP in Downloads. A bad or tampered file changes nothing. Only the hub and its command line are replaced; installed apps stay as they are and update from their cards. The old hub is parked as `voidflare.old.exe` and removed at the next start. (A 0.1.0 or 0.2.0 hub still only downloads: update those once by hand.)

### Fixed

- The hub reported itself as version 0.1.0 (its own `Cargo.toml` still said so). It now takes its version from the one place every program takes it from, and `release.ps1` refuses to build a release if any program says another version or the changelog has no section for it.

## [0.2.0] - 2026-11-01

### Added

- **Install only what you need.** The hub is now a small download (about 7 MB). Each app is its own pack: *Install*, *Update* and *Remove* on its card, with a progress bar and *Cancel*. The card says before you click what will be downloaded and how much. Every pack is checked against the release's `SHA256SUMS.txt` (and the pack list `manifest.json` is itself listed there) before it is installed; a failed or cancelled install leaves the previous one untouched.
- **No duplicate engines.** If your PC already has an ArtCraft engine (WordCraft, GridCraft, DeckCraft, PdfCraft) in a version we have tested (`src/engine-compat.json`), the app uses it and does not download a second copy. Any other version gets our own tested copy. The first tested versions are WordCraft 0.12.0 and 0.13.2, GridCraft 0.13.0 and 0.14.2, DeckCraft 0.12.0 and 0.13.1, PdfCraft 0.15.0 and 0.16.1. DWG to PDF always brings its own CADCraft.
- If ArtCraft updates after you installed an app, the card shows *Engine untested* with a *Bundle engine* button, or *Reinstall* if the engine is gone.
- `cargo test -p vf-core --test engine_compat` runs one real conversion per engine with the pinned copy and the installed one, and prints the matrix behind that list.
- The hub updates itself from the small hub ZIP instead of the full bundle.

### Changed

- A release now carries: the full bundle (`voidflare-<version>-windows.zip`, same name and layout as before, for a PC without a network and for 0.1.0 hubs), the hub alone (`voidflare-hub-…`), one ZIP per app and per engine (`voidflare-app-…`, `voidflare-engine-…`), `manifest.json` and `SHA256SUMS.txt`.
- The website offers the hub and the full bundle side by side, each with its SHA-256.
- The hub's cards are taller, the list view no longer overlaps its columns, and the footer no longer draws over itself in a narrow window.

## [0.1.0] - 2026-10-30

First public release. The programs are not code-signed.

### Added

- The hub with a drop panel: drop or choose a file, then pick the format it should become.
- Five apps: Word to PDF, Excel to PDF, Slides to PDF, PDF Tools, DWG to PDF, each with its own engine.
- English, Italiano, Español, Français and Deutsch.
- The Journal-style torch and dot grid in the hub.
- Updates from GitHub: the hub checks the latest release once at start (switch it off with *AUTO-CHECK* in the footer) or when you click *CHECK FOR UPDATES*, and downloads the ZIP into Downloads after checking its SHA-256 against `SHA256SUMS.txt`. It never installs anything by itself.
- The website at [voidflare.app](https://voidflare.app).
