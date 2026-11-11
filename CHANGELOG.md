# Changelog

All notable changes to SwapCraft (formerly voidflare) are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.6.0] - 2026-11-11

### Added

- **SwapCraft for Linux** (64-bit, x86_64). The hub and all nine apps run on Linux, with the official Linux builds of the ArtCraft engines at the same versions as on Windows (CADCraft built from source with our patch, as on Windows). Download `swapcraft-hub-<version>-linux-x86_64.zip`, unzip it and run `swapcraft`, then install the converters you need from their cards; or take `swapcraft-<version>-linux-x86_64.zip`, which has everything in one file. Settings can put SwapCraft in your applications menu and on the desktop. Your settings and recipes are kept in `~/.config/voidflare`. The command line (`swapcraft-cli`) and the MCP server work as on Windows. Built on Ubuntu 22.04, so it needs a system at least that recent (Linux Mint 21 or newer, for example).
- **Convert with SwapCraft in Nemo**, Linux Mint's file manager. In Settings, *Right-click menu* adds the entry: right-click one or more files, choose Convert with SwapCraft, then pick a format, a recipe or an app. *Convert here with SwapCraft* sits on the empty space of a folder: copy files with Ctrl+C, open the destination folder, right-click its empty space and choose it, and the results land there. The names are in all five languages. Nemo has no way to add to the menu of a drag with the right mouse button, so that stays a Windows File Explorer feature.
- **A picture can become a PDF.** PNG, JPG, TIFF, BMP, GIF, WebP and the other picture types become a PDF with one page the size of the picture, from the hub, the right-click menus, Convert here, the command line and recipes. A recipe that starts with OCR, shrink, merge, split or extract now takes pictures too: *Scans to searchable PDF* works on a photo of a page, and *Report pack* on a folder of pictures makes one PDF with a page for each.
- **Text, web pages and Markdown can become PDF and Word** (before, only EPUB) from the hub, the menus and the command line. Plain text keeps its line breaks and has no title page.
- **A file with no extension is recognised by its content** (text, PDF, Office files, EPUB, pictures, audio, video, DWG), which matters on Linux, where editors save a note as `notes`. When it cannot tell, it says so instead of "nothing to convert".
- **A progress card** for what you start from the right-click menu, in the bottom right corner: the butterfly, *Converting 3 files to PDF…*, a thin bar, then *Done: 3 PDF in Exports* with *Open folder*. It closes by itself after a few seconds; if a file failed it stays, with *Details*.
- **Cancel stops the engine at once**, also in the middle of a long video, and leaves no half-made file.
- **One SwapCraft at a time.** Starting it again brings the open window forward, and files dropped on the program or sent with *Open with* arrive in it.

### Changed

- **SwapCraft 0.5.0 did not start on some Windows PCs.** With certain Intel graphics drivers (seen with an Intel UHD 620, driver 31.0.101.2135), it crashed inside the driver's Vulkan code before showing a window. It now draws with Direct3D 12 only, which every Windows 10 and 11 has. The `WGPU_BACKEND` environment variable still overrides it.
- **Audio keeps the quality of the file it comes from** when converted from the hub, the menus and the command line: 16-bit files and MP3 or OGG stay 16-bit instead of becoming 24-bit.
- **Language and appearance are chosen with buttons** in Settings, not drop-downs, and the change shows at once. A message already on screen follows a change of language, and the File Explorer menu is written again in the new language.
- **Windows open centred and inside the screen.** In the apps, the log is above the Convert and Cancel buttons, which no longer jump when a run starts. The format offered first is no longer the one the file already is (PNG to PNG, EPUB to EPUB). *Add files* now says when every picked file was refused, as dropping did.
- **Smaller things:** *Check for updates* shows *Checking…* long enough to be seen; Grid or List and the sort order are remembered; a long message in the footer is cut with … instead of covering the update controls; a very long file name is shown on one line; a result next to a name near the 255-byte limit gets its number without failing; a file that is not a DWG says so instead of "unsupported file type"; an error from an engine no longer starts with a second "error:"; `swapcraft-cli <command> --help` prints the help and `swapcraft-cli engines` lists all eight engines.
- **On Linux, results are created with the permissions of your other files**, not readable by you alone, so a shared folder or a network share shows them to the group.
- **The ArtCraft engines:** DeckCraft 0.18.0 and FilmCraft 0.20.0 (both released on 11 November 2026, mostly hardening against damaged files), the others unchanged.

### Known limits

- On Linux, the menu entries are for Nemo only; other file managers do not show them, and there is no right-mouse-button drag.
- When only the sound of a video is taken, FilmCraft 0.20 writes stereo even if the source sound is mono.
- Not yet checked on a real File Explorer after these changes: the right-click menu and the drag with the right mouse button on Windows (they were last checked with 0.5.0), and on one test PC, apps opened from the hub were seen to start minimized, with the cause not confirmed; if it happens, use the taskbar button.

## [0.5.0] - 2026-11-10

### Changed

- **SwapCraft is now the name of the files too.** The first time the new hub starts, it renames itself from `voidflare.exe` to `SwapCraft.exe` and the command line from `voidflare-cli.exe` to `swapcraft-cli.exe`, replaces a `voidflare.lnk` Start menu shortcut with `SwapCraft.lnk`, writes the File Explorer menus again for the new name, and tells you once. Your settings, recipes and the `%APPDATA%\voidflare` folder stay as they are. Windows ties a pin to the file name, so if you had pinned it to Start or to the taskbar, pin it again from there. **If you added the MCP server to an AI assistant by hand, change its path to `swapcraft-cli.exe`** (the tool names stay the same).
- **A new look.** A black theme is now the default for everyone, and you can switch to Same as Windows, Dark or Light in Settings. The text is set in Inter and IBM Plex Mono, the hub has a new butterfly icon, and an app's card lights up in that app's colour under the pointer. The torch light and the dot grid are gone.
- **Format names are translated.** Names such as *PDF · sheets, merged* or *PNG pictures* now follow your language in the hub, in Recipes and in the File Explorer menus. The command line and the MCP server keep the English names, so scripts do not change.
- **The ArtCraft engines are updated to the latest releases:** WordCraft 0.17.0, GridCraft 0.18.0, DeckCraft 0.17.0, PdfCraft 0.20.0, PhotoCraft 0.22.0, SoundCraft 0.19.0 and FilmCraft 0.19.0. Among the engines' own fixes: WordCraft opens encrypted `.docx` files again and keeps non-Unicode paths on the command line, and PdfCraft keeps ruled tables when it exports a PDF to Word. Each engine passed the engine test with the bundled copy and the installed one.

### Added

- **SwapCraft on the desktop.** Settings has a button that puts a SwapCraft shortcut on your desktop, and takes it away again.

## [0.4.1] - 2026-11-08

### Added

- **Convert here, by dragging.** Drag files with the right mouse button and drop them on a folder: next to *Copy here* and *Move here*, File Explorer now offers *Convert here with SwapCraft*, with the conversions and recipes that fit every dragged file. The results go to the folder you dropped on, and the originals stay where they were. It comes with the right-click menu (Settings > Right-click menu), for your Windows user only, and removing that menu removes this too. It works on Windows 11 without *Show more options*, because the drag menu is still the classic one. A hub that already has the right-click menu adds it by itself the first time it starts.

### Changed

- **The converter is now called SwapCraft, by voidflare.** voidflare stays the name of the maker and of its other programs; this one is SwapCraft. The repository moves to `github.com/Flaeren/swapcraft` (the old address keeps working) and the website to `swapcraft.voidflare.app`. The hub, its window, the Start menu entry for new installs and the File Explorer menus (*Convert with SwapCraft*) use the new name; a menu you added earlier is renamed by itself the first time the hub starts. File and folder names stay as they were, so nothing you installed moves: `voidflare.exe`, `voidflare-cli.exe`, the `%APPDATA%\voidflare` folder, the MCP tool names, and a Start menu shortcut you already have (renaming it would undo your pin).
- **The ArtCraft engines are updated to the latest releases:** WordCraft 0.16.0, GridCraft 0.17.0, DeckCraft 0.16.0, PdfCraft 0.19.0, PhotoCraft 0.21.0, SoundCraft 0.18.0 and FilmCraft 0.18.0. They bring the engines' own fixes, among them stricter checks on damaged or hostile files (PdfCraft encryption and JBIG2 images, PhotoCraft PSD and EXR memory limits, FilmCraft AVI, MP4 and DNxHD). Each passed the engine test with the bundled copy and the installed one.

## [0.4.0] - 2026-11-06

### Added

- **Recipes.** A new RECIPES tab in the hub: name a chain of steps (convert, merge into one PDF, shrink, OCR, split, extract pages) and run it on files in one go, for example *DWG to PDF sheets, merge everything, make it smaller*. Each step is something the apps already do, so the result is the same as doing the steps by hand; the files in between stay in a scratch folder and only the last ones are saved, next to the first file or in a folder you choose, with a name you choose (`{name}`, `{folder}`, `{recipe}`), never over an existing file. The tab shows which file types a recipe takes and says when no type can go through its steps. Three example recipes are there on the first start. When you drop a file, the recipes that take it are offered under the formats.
- **Right-click menu in File Explorer** (Settings > Right-click menu): *Convert with voidflare* on every type voidflare converts, with that type's conversions, the recipes that take it, and *Open in* the app with all the options. With several files selected they are converted together, in one small window that shows the progress and opens the folder at the end. It is added for your Windows user only, no administrator needed, and removing it takes away everything it added. On Windows 11 it is under *Show more options*: the short menu only takes signed, packaged apps. `voidflare.exe --add-menu` and `--remove-menu` do the same from a script.
- **Recipes from the command line and the MCP server:** `voidflare-cli recipes` lists them, `voidflare-cli recipe "<name>" <files or folders>` runs one; the MCP server has `voidflare_recipes` and `voidflare_recipe_run`, and a recipe that saves to a fixed folder may only do so inside the folders the server was allowed.

### Changed

- **The hub's torch light is off by default.** The dots still swell and part around the pointer; the glow under it and on the borders can be turned back on in Settings > Appearance (*Torch light under the pointer*).
- **The ArtCraft engines are updated to the latest releases:** WordCraft 0.15.2, GridCraft 0.16.1, DeckCraft 0.15.1, PdfCraft 0.18.2, PhotoCraft 0.20.2, SoundCraft 0.17.1 and FilmCraft 0.17.1. Most are the same code as before, released again with signed programs; WordCraft 0.15.1 and PdfCraft 0.18.1 also bring fixes (Word import, smaller pictures under masks when shrinking a PDF). Each passed the engine test with the bundled copy and the installed one, and a real 127-page book and a spreadsheet were converted with the new bundled engines.

## [0.3.5] - 2026-11-03

### Added

- **The hub speaks your language.** The hub's own texts (the cards, the drop panel, the footer, the settings and the welcome dialog) are now in English, Italian, Spanish, French and German, following the language chosen in the settings; the change shows at once in the hub, and in the converter apps the next time they start. The translations have not been reviewed by native speakers yet; the names of the Windows menu entries in the pinning instructions are written from memory.

### Changed

- **The ArtCraft engines are updated to the latest releases:** WordCraft 0.15.0 (was 0.12.0), GridCraft 0.16.0 (0.13.0), DeckCraft 0.15.0 (0.12.0), PdfCraft 0.18.0 (0.15.0), PhotoCraft 0.20.1 (0.19.0), SoundCraft 0.17.0 (0.16.0) and FilmCraft 0.17.0 (0.16.0). Each passed the engine test (one real conversion per engine, with the bundled copy and with the installed one) and is now on the list of versions an app may reuse from your PC.

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
