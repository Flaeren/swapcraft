# Changelog

All notable changes to voidflare are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
