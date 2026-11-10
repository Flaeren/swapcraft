//! Looking for a newer voidflare on the project's GitHub Releases, and downloading it with its checksum
//! verified. Nothing here runs by itself: the hub calls it only when the user asks, and nothing is sent
//! beyond the plain HTTPS requests below (no identifier, no telemetry).
//!
//! A release is expected to carry the portable ZIP and a `SHA256SUMS.txt` (lines `<hash>  <file name>`).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const REPO: &str = "Flaeren/swapcraft";
const SUMS_NAME: &str = "SHA256SUMS.txt";

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Without the leading `v`.
    pub version: String,
    /// The release page, for the browser.
    pub page: String,
    pub notes: String,
    pub zip: Option<Asset>,
    pub sums: Option<Asset>,
    /// Every file the release carries (packs, engines, manifest), for the hub's pack installer.
    pub assets: Vec<Asset>,
}

impl Release {
    /// What the hub downloads to update itself: the small hub-only ZIP if the release has one, else the full bundle.
    /// Since 0.5 a release carries two hub ZIPs: `swapcraft-hub-*` (SwapCraft.exe), which this hub takes, and
    /// `voidflare-hub-*` (the same program as voidflare.exe), for hubs up to 0.4, which look for that name.
    pub fn update_zip(&self) -> Option<&Asset> {
        let hub = |prefix: &str| self.assets.iter().find(|a| a.name.starts_with(prefix) && a.name.ends_with("-windows.zip"));
        hub("swapcraft-hub-").or_else(|| hub("voidflare-hub-")).or(self.zip.as_ref())
    }

    /// The release file with exactly this name.
    pub fn find_asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

fn settings_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("voidflare").join("updates.txt"))
}

/// Whether the hub looks for a newer release by itself when it starts. On unless the user turned it off
/// (the file holds `off`) or `VOIDFLARE_NO_UPDATE_CHECK` is set.
pub fn auto_enabled() -> bool {
    if std::env::var_os("VOIDFLARE_NO_UPDATE_CHECK").is_some() {
        return false;
    }
    settings_file().and_then(|p| fs::read_to_string(p).ok()).is_none_or(|s| s.trim() != "off")
}

pub fn set_auto(on: bool) {
    if let Some(path) = settings_file() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, if on { "on\n" } else { "off\n" });
    }
}

fn numbers(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|part| part.parse::<u64>().ok())
        .collect()
}

/// Whether `latest` is a higher version than `current` (`0.2.0` > `0.1.9`; a missing part counts as 0).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (numbers(latest), numbers(current));
    if a.is_empty() {
        return false;
    }
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// Reads the JSON the GitHub API returns for `releases/latest`.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("The release information could not be read: {e}"))?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let tag = text("tag_name");
    if tag.is_empty() {
        return Err("The release information has no version.".to_owned());
    }
    let assets: Vec<Asset> = v
        .get("assets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a.get("name")?.as_str()?.to_owned(),
                        url: a.get("browser_download_url")?.as_str()?.to_owned(),
                        size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        page: text("html_url"),
        notes: text("body"),
        // The full offline bundle: `swapcraft-<version>-windows.zip` since 0.5, `voidflare-<version>-windows.zip` before.
        // A hub that sees several ZIPs prefers it over the small hub-only and per-app files.
        zip: ["swapcraft-", "voidflare-"]
            .iter()
            .find_map(|prefix| {
                let hub = format!("{prefix}hub-");
                assets.iter().find(|a| a.name.to_lowercase().ends_with("-windows.zip") && a.name.starts_with(prefix) && !a.name.starts_with(&hub) && !a.name.contains("-app-") && !a.name.contains("-engine-"))
            })
            .or_else(|| assets.iter().find(|a| a.name.to_lowercase().ends_with(".zip")))
            .cloned(),
        sums: assets.iter().find(|a| a.name == SUMS_NAME).cloned(),
        assets,
    })
}

fn user_agent() -> String {
    format!("voidflare/{}", env!("CARGO_PKG_VERSION"))
}

fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(404) => "No release has been published yet.".to_owned(),
        ureq::Error::StatusCode(403 | 429) => "GitHub is limiting requests from this connection. Try again later.".to_owned(),
        ureq::Error::StatusCode(code) => format!("GitHub answered with error {code}."),
        other => format!("Could not reach GitHub: {other}"),
    }
}

/// Asks GitHub for the latest published release.
pub fn latest() -> Result<Release, String> {
    // `VOIDFLARE_RELEASES_URL` points the check at another URL that answers like the GitHub API (a local test
    // server). Only the user who starts the program can set it, so it adds no trust that they did not already have.
    let url = std::env::var("VOIDFLARE_RELEASES_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let body = ureq::get(&url)
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json")
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .call()
        .map_err(http_error)?
        .body_mut()
        .read_to_string()
        .map_err(http_error)?;
    parse_release(&body)
}

/// The hash listed for `file` in a `SHA256SUMS.txt` (`<hash>  <name>` or `<hash> *<name>` per line).
pub fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file && hash.len() == 64).then(|| hash.to_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads the release ZIP into `dest_dir`, checks it against the published SHA-256 and only then gives it its
/// final name. A file that does not match is deleted. `progress` gets (bytes so far, total).
pub fn download_verified(release: &Release, dest_dir: &Path, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let zip = release.update_zip().ok_or("This release has no ZIP to download.")?;
    let sums_text = sums_text(release)?;
    let expected = checksum_for(&sums_text, &zip.name).ok_or_else(|| format!("{SUMS_NAME} does not list {}.", zip.name))?;
    fetch_verified(zip, &expected, dest_dir, progress, cancel)
}

// ---------------------------------------------------------------- installing the new hub

/// Folder next to the hub where the update is downloaded and unpacked; it is removed afterwards.
const STAGING: &str = ".voidflare-update";

/// Where the running hub is parked while its replacement takes its name (`voidflare.exe` -> `voidflare.old.exe`;
/// the drag-and-drop DLL, which Explorer may have loaded, likewise -> `voidflare_dragdrop.old.dll`).
/// Windows lets a running program or a loaded DLL be renamed but not overwritten.
pub fn old_name(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "voidflare".to_owned());
    let ext = exe.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "exe".to_owned());
    exe.with_file_name(format!("{stem}.old.{ext}"))
}

/// Downloads the release ZIP, checks its SHA-256, and replaces the running hub with the `voidflare.exe` inside it.
/// Only that one file is taken: the apps next to the hub are never touched (each updates from its own card), even
/// when the ZIP is the full bundle. Returns the path of the new hub, which the caller starts. On any failure the
/// hub that is running stays exactly as it was.
pub fn install_hub(release: &Release, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot tell where voidflare is installed: {e}"))?;
    install_hub_at(&exe, release, progress, cancel)
}

/// `install_hub` for the hub program at `exe` (the running one, or a copy in a test).
pub fn install_hub_at(exe: &Path, release: &Release, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let dir = exe.parent().ok_or("Cannot tell which folder voidflare is in.")?.to_path_buf();
    let staging = dir.join(STAGING);
    let _ = fs::remove_dir_all(&staging);
    let result = download_verified(release, &staging, progress, cancel).and_then(|zip| install_from_zip(&zip, exe, &staging));
    let _ = fs::remove_dir_all(&staging);
    result.map(|()| exe.to_path_buf())
}

/// Unpacks `zip` inside `work` and puts its hub (`SwapCraft.exe`, or `voidflare.exe` in a ZIP made before 0.5)
/// in place of `exe`.
pub fn install_from_zip(zip: &Path, exe: &Path, work: &Path) -> Result<(), String> {
    let unpacked = work.join("unpacked");
    crate::packs::extract_zip(zip, &unpacked, &AtomicBool::new(false))?;
    let new = [HUB_NAME, LEGACY_HUB_NAME].iter().map(|n| unpacked.join(n)).find(|p| is_exe(p));
    let Some(new) = new else {
        return Err("The update has no usable SwapCraft.exe; nothing was changed.".to_owned());
    };
    replace_exe(exe, &new)?;
    // The command-line program (and MCP server) travels with the hub: replace it too when the update has one.
    // Best effort: an MCP client may be holding it, and a stale command line never stops the hub.
    let new_cli = [CLI_NAME, LEGACY_CLI_NAME].iter().map(|n| unpacked.join(n)).find(|p| is_exe(p)).unwrap_or_else(|| unpacked.join(CLI_NAME));
    if is_exe(&new_cli) {
        let cli = exe.with_file_name(CLI_NAME);
        if cli.exists() {
            let _ = replace_exe(&cli, &new_cli);
        } else {
            let _ = fs::rename(&new_cli, &cli);
        }
    }
    // So does the drag-and-drop DLL (same name, so the registry entry stays right). Explorer may have it loaded:
    // the old one is parked under a new name and Explorer picks up the new one the next time it loads it. Best
    // effort: an old DLL keeps working with the new hub.
    let dll = exe.with_file_name(DRAG_DLL);
    let new_dll = unpacked.join(DRAG_DLL);
    if is_exe(&new_dll) {
        if dll.exists() {
            let _ = replace_exe(&dll, &new_dll);
        } else {
            let _ = fs::rename(&new_dll, &dll);
        }
    }
    Ok(())
}

/// The drag-and-drop menu handler that sits beside the hub.
pub const DRAG_DLL: &str = voidflare_dragdrop::config::DLL_NAME;

/// The hub program.
pub const HUB_NAME: &str = "SwapCraft.exe";
/// Its name up to 0.4.
pub const LEGACY_HUB_NAME: &str = "voidflare.exe";
/// The command-line program that sits beside the hub.
pub const CLI_NAME: &str = "swapcraft-cli.exe";
/// Its name up to 0.4.
pub const LEGACY_CLI_NAME: &str = "voidflare-cli.exe";

/// 0.5 renames the programs: a hub that starts as `voidflare.exe` gives itself the name `SwapCraft.exe`, and
/// `voidflare-cli.exe` next to it becomes `swapcraft-cli.exe`. Windows lets a running program be renamed. Returns the
/// new path of the hub when it was renamed (the caller starts it and ends), None when there was nothing to do. A
/// failure leaves the hub where it was: it keeps working under the old name and tries again at the next start.
pub fn rename_legacy(exe: &Path) -> Result<Option<PathBuf>, String> {
    let is_legacy = exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(LEGACY_HUB_NAME));
    if !is_legacy {
        return Ok(None);
    }
    let target = exe.with_file_name(HUB_NAME);
    if target.exists() {
        // an older copy of the new name (or a half-done rename): the running program is the one to keep
        fs::remove_file(&target).map_err(|e| format!("Cannot replace {}: {e}", target.display()))?;
    }
    fs::rename(exe, &target).map_err(|e| format!("Cannot rename {}: {e}", exe.display()))?;
    let (old_cli, new_cli) = (exe.with_file_name(LEGACY_CLI_NAME), exe.with_file_name(CLI_NAME));
    if old_cli.exists() {
        // best effort: an MCP client may be holding the old one
        if !new_cli.exists() || fs::remove_file(&new_cli).is_ok() {
            let _ = fs::rename(&old_cli, &new_cli);
        }
    }
    Ok(Some(target))
}

fn is_exe(p: &Path) -> bool {
    let mut head = [0u8; 2];
    fs::File::open(p).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"MZ"
}

/// `exe` is renamed to its `.old.exe` name, `new` takes its place; if that fails the old one is put back.
pub fn replace_exe(exe: &Path, new: &Path) -> Result<(), String> {
    let old = old_name(exe);
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old).map_err(|e| format!("Cannot replace {} (is the folder read-only?): {e}", exe.display()))?;
    if let Err(e) = fs::rename(new, exe) {
        let _ = fs::rename(&old, exe);
        return Err(format!("Cannot put the new voidflare in place, the old one was kept: {e}"));
    }
    Ok(())
}

/// Removes what a finished update leaves behind: the parked old hub (it may still be closing, so the caller
/// may try again a moment later) and a half-unpacked staging folder.
pub fn cleanup_after_update() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(old_name(&exe));
        let _ = fs::remove_file(old_name(&exe.with_file_name(CLI_NAME)));
        // fails while Explorer still has the old DLL loaded; a later start removes it
        let _ = fs::remove_file(old_name(&exe.with_file_name(DRAG_DLL)));
        if let Some(dir) = exe.parent() {
            let _ = fs::remove_dir_all(dir.join(STAGING));
        }
    }
}

/// The text of the release's `SHA256SUMS.txt`.
pub fn sums_text(release: &Release) -> Result<String, String> {
    let sums = release.sums.as_ref().ok_or("This release has no checksum file, so it cannot be verified.")?;
    fetch_text(&sums.url)
}

/// A small text file (the checksums, the pack manifest) by plain HTTPS GET.
pub fn fetch_text(url: &str) -> Result<String, String> {
    ureq::get(url)
        .header("User-Agent", user_agent())
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .call()
        .map_err(http_error)?
        .body_mut()
        .read_to_string()
        .map_err(http_error)
}

/// Downloads one release file into `dest_dir`, checks it against `expected` (a lower-case SHA-256) and only then
/// gives it its final name. A file that does not match, or a cancelled download, leaves nothing behind.
pub fn fetch_verified(zip: &Asset, expected: &str, dest_dir: &Path, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    // `Path::file_name` keeps only the last part, so a hostile name cannot point outside `dest_dir`.
    let safe_name = Path::new(&zip.name).file_name().ok_or("The release file has no usable name.")?;
    fs::create_dir_all(dest_dir).map_err(|e| format!("Cannot create {}: {e}", dest_dir.display()))?;
    let part = dest_dir.join(format!("{}.part", safe_name.to_string_lossy()));
    let target = dest_dir.join(safe_name);

    let result = (|| -> Result<(), String> {
        let response = ureq::get(&zip.url).header("User-Agent", user_agent()).call().map_err(http_error)?;
        let total = response.body().content_length().unwrap_or(zip.size);
        let mut reader = response.into_body().into_with_config().limit(8 * 1024 * 1024 * 1024).reader();
        let mut file = fs::File::create(&part).map_err(|e| format!("Cannot write {}: {e}", part.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Download cancelled.".to_owned());
            }
            let n = reader.read(&mut buf).map_err(|e| format!("The download was interrupted: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| format!("Cannot write {}: {e}", part.display()))?;
            done += n as u64;
            progress(done, total);
        }
        file.flush().map_err(|e| e.to_string())?;
        let actual = hex(hasher.finalize().as_slice());
        if actual != expected.to_lowercase() {
            return Err("The downloaded file does not match its published checksum, so it was discarded.".to_owned());
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            let _ = fs::remove_file(&target);
            fs::rename(&part, &target).map_err(|e| format!("Cannot save {}: {e}", target.display()))?;
            Ok(target)
        }
        Err(msg) => {
            let _ = fs::remove_file(&part);
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "tag_name": "v0.2.0",
      "html_url": "https://github.com/Flaeren/voidflare/releases/tag/v0.2.0",
      "body": "Fixes.",
      "assets": [
        {"name": "voidflare-0.2.0-windows.zip", "browser_download_url": "https://example.test/z.zip", "size": 450000000},
        {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/s.txt", "size": 120}
      ]
    }"#;

    #[test]
    fn versions_compare_by_number_not_text() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v0.10.0", "0.9.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn a_release_is_read_from_the_github_json() {
        let r = parse_release(SAMPLE).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.zip.as_ref().unwrap().name, "voidflare-0.2.0-windows.zip");
        assert_eq!(r.zip.as_ref().unwrap().size, 450_000_000);
        assert_eq!(r.sums.as_ref().unwrap().name, "SHA256SUMS.txt");
        assert!(parse_release("{}").is_err());
        assert!(parse_release("not json").is_err());
    }

    #[test]
    fn with_several_zips_the_full_bundle_is_the_update_file() {
        // The small hub and the per-app ZIPs come first on purpose: a 0.1.0 hub must still pick the full bundle.
        let json = r#"{"tag_name": "v0.2.0", "assets": [
          {"name": "voidflare-app-word-to-pdf-0.2.0-windows-x64.zip", "browser_download_url": "https://e.test/a.zip", "size": 1},
          {"name": "voidflare-engine-wordcraft-0.12.0-windows-x64.zip", "browser_download_url": "https://e.test/e.zip", "size": 1},
          {"name": "voidflare-hub-0.2.0-windows.zip", "browser_download_url": "https://e.test/h.zip", "size": 1},
          {"name": "voidflare-0.2.0-windows.zip", "browser_download_url": "https://e.test/f.zip", "size": 1},
          {"name": "manifest.json", "browser_download_url": "https://e.test/m.json", "size": 1},
          {"name": "SHA256SUMS.txt", "browser_download_url": "https://e.test/s.txt", "size": 1}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.zip.as_ref().unwrap().name, "voidflare-0.2.0-windows.zip");
        assert_eq!(r.assets.len(), 6);
        assert_eq!(r.update_zip().unwrap().name, "voidflare-hub-0.2.0-windows.zip", "the hub updates itself from the small ZIP");
        assert!(r.find_asset("manifest.json").is_some() && r.find_asset("nope.zip").is_none());
        // No bundle at all: fall back to any ZIP, as before.
        let only_app = r#"{"tag_name": "v0.2.0", "assets": [{"name": "a.zip", "browser_download_url": "https://e.test/a.zip", "size": 1}]}"#;
        assert_eq!(parse_release(only_app).unwrap().zip.unwrap().name, "a.zip");
    }

    #[test]
    fn the_checksum_is_found_by_file_name() {
        let h = "a".repeat(64);
        let sums = format!("{h}  voidflare-0.2.0-windows.zip\n{} *other.zip\n", "b".repeat(64));
        assert_eq!(checksum_for(&sums, "voidflare-0.2.0-windows.zip"), Some(h));
        assert_eq!(checksum_for(&sums, "other.zip"), Some("b".repeat(64)));
        assert_eq!(checksum_for(&sums, "missing.zip"), None);
        assert_eq!(checksum_for("short  x.zip", "x.zip"), None);
    }

    /// End to end against a local web server: `python -m http.server 8765` in an empty folder, then
    /// `VF_UPDATE_DIR=<that folder> cargo test --release -p vf-core served_zip -- --ignored`.
    #[test]
    #[ignore = "needs a local web server, see the comment"]
    fn served_zip_is_verified() {
        let dir = PathBuf::from(std::env::var("VF_UPDATE_DIR").expect("VF_UPDATE_DIR"));
        let base = "http://127.0.0.1:8765";
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        fs::write(dir.join("fake.zip"), &bytes).unwrap();
        let good = hex(Sha256::digest(&bytes).as_slice());
        fs::write(dir.join("SHA256SUMS.txt"), format!("{good}  fake.zip\n")).unwrap();
        fs::write(dir.join("SUMS-BAD.txt"), format!("{}  fake.zip\n", "0".repeat(64))).unwrap();
        let release = |sums: &str| Release {
            version: "9.9.9".into(),
            page: String::new(),
            notes: String::new(),
            zip: Some(Asset { name: "fake.zip".into(), url: format!("{base}/fake.zip"), size: bytes.len() as u64 }),
            sums: Some(Asset { name: "SHA256SUMS.txt".into(), url: format!("{base}/{sums}"), size: 0 }),
            assets: Vec::new(),
        };
        let out = dir.join("downloads");
        let seen = std::cell::Cell::new(0u64);
        let cancel = AtomicBool::new(false);

        let saved = download_verified(&release("SHA256SUMS.txt"), &out, &|done, _| seen.set(done), &cancel).expect("a matching file is accepted");
        assert_eq!(fs::read(&saved).unwrap(), bytes);
        assert_eq!(seen.get(), bytes.len() as u64);
        assert!(!out.join("fake.zip.part").exists());

        fs::remove_file(&saved).unwrap();
        let err = download_verified(&release("SUMS-BAD.txt"), &out, &|_, _| {}, &cancel).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
        assert!(!out.join("fake.zip").exists() && !out.join("fake.zip.part").exists(), "nothing is left behind");
    }

    /// Against the real GitHub release (about 210 MB): `cargo test --release -p vf-core real_release -- --ignored --nocapture`.
    #[test]
    #[ignore = "downloads the published release"]
    fn real_release_is_found_and_verified() {
        let release = latest().expect("the latest release is readable");
        println!("latest: {} ({})", release.version, release.page);
        assert!(release.zip.is_some() && release.sums.is_some(), "the release carries the ZIP and SHA256SUMS.txt");
        let dir = std::env::temp_dir().join("voidflare-update-test");
        let _ = fs::remove_dir_all(&dir);
        let saved = download_verified(&release, &dir, &|_, _| {}, &AtomicBool::new(false)).expect("the real download verifies");
        println!("saved: {} ({} bytes)", saved.display(), fs::metadata(&saved).unwrap().len());
        assert!(is_newer("9.9.9", &release.version) && !is_newer(&release.version, &release.version));
        let _ = fs::remove_dir_all(&dir);
    }

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn the_hub_is_replaced_and_the_apps_are_left_alone() {
        let root = std::env::temp_dir().join(format!("vf-hub-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("apps/word")).unwrap();
        fs::write(root.join("apps/word/word.exe"), b"MZ-app-keeps-this").unwrap();
        let exe = root.join("SwapCraft.exe");
        fs::write(&exe, b"MZ-old-hub").unwrap();
        let zip = root.join("update.zip");
        // a full bundle: the hub plus an app that must NOT be taken
        zip_with(&zip, &[("SwapCraft.exe", b"MZ-new-hub"), ("swapcraft-cli.exe", b"MZ-new-cli"), ("apps/word/word.exe", b"MZ-new-app"), ("README.txt", b"hi")]);
        fs::write(root.join("swapcraft-cli.exe"), b"MZ-old-cli").unwrap();

        install_from_zip(&zip, &exe, &root.join(".work")).expect("the swap works");
        assert_eq!(fs::read(&exe).unwrap(), b"MZ-new-hub");
        assert_eq!(fs::read(root.join("SwapCraft.old.exe")).unwrap(), b"MZ-old-hub", "the old hub is parked, not lost");
        assert_eq!(fs::read(root.join("apps/word/word.exe")).unwrap(), b"MZ-app-keeps-this", "installed apps are untouched");
        assert!(!root.join("README.txt").exists(), "nothing but the hub (and its command line) is copied out");
        assert_eq!(fs::read(root.join("swapcraft-cli.exe")).unwrap(), b"MZ-new-cli", "the command line travels with the hub");
        assert_eq!(fs::read(root.join("swapcraft-cli.old.exe")).unwrap(), b"MZ-old-cli");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_made_before_0_5_still_updates_the_hub() {
        let root = std::env::temp_dir().join(format!("vf-hub-legacy-zip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("SwapCraft.exe");
        fs::write(&exe, b"MZ-old-hub").unwrap();
        let zip = root.join("update.zip");
        zip_with(&zip, &[("voidflare.exe", b"MZ-new-hub"), ("voidflare-cli.exe", b"MZ-new-cli")]);
        install_from_zip(&zip, &exe, &root.join(".work")).expect("the swap works");
        assert_eq!(fs::read(&exe).unwrap(), b"MZ-new-hub");
        assert_eq!(fs::read(root.join("swapcraft-cli.exe")).unwrap(), b"MZ-new-cli", "the old command line arrives under the new name");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn voidflare_exe_renames_itself_to_swapcraft_exe() {
        let root = std::env::temp_dir().join(format!("vf-rename-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let old = root.join("voidflare.exe");
        fs::write(&old, b"MZ-hub").unwrap();
        fs::write(root.join("voidflare-cli.exe"), b"MZ-cli").unwrap();
        fs::write(root.join("SwapCraft.exe"), b"MZ-stale").unwrap();
        let new = rename_legacy(&old).expect("renamed").expect("there was something to do");
        assert_eq!(new, root.join("SwapCraft.exe"));
        assert_eq!(fs::read(&new).unwrap(), b"MZ-hub", "the running program wins over a stale copy of the new name");
        assert!(!old.exists() && !root.join("voidflare-cli.exe").exists());
        assert_eq!(fs::read(root.join("swapcraft-cli.exe")).unwrap(), b"MZ-cli");
        assert_eq!(rename_legacy(&new).unwrap(), None, "a hub already called SwapCraft.exe is left alone");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_new_hub_zip_wins_over_the_legacy_one() {
        let json = r#"{"tag_name": "v0.5.0", "html_url": "https://e.test/r", "assets": [
          {"name": "voidflare-app-word-to-pdf-0.5.0-windows-x64.zip", "browser_download_url": "https://e.test/a.zip", "size": 1},
          {"name": "voidflare-hub-0.5.0-windows.zip", "browser_download_url": "https://e.test/old.zip", "size": 1},
          {"name": "swapcraft-hub-0.5.0-windows.zip", "browser_download_url": "https://e.test/new.zip", "size": 1},
          {"name": "swapcraft-0.5.0-windows.zip", "browser_download_url": "https://e.test/full.zip", "size": 1}
        ]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.update_zip().unwrap().name, "swapcraft-hub-0.5.0-windows.zip");
        assert_eq!(r.zip.as_ref().unwrap().name, "swapcraft-0.5.0-windows.zip");
    }

    /// A tiny web server on a free local port: every path in `routes` answers with its bytes, anything else is 404.
    fn serve(routes: Vec<(String, Vec<u8>)>) -> String {
        use std::io::BufRead;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut line = String::new();
                {
                    let mut reader = std::io::BufReader::new(&stream);
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    let mut header = String::new();
                    while reader.read_line(&mut header).map(|n| n > 2).unwrap_or(false) {
                        header.clear();
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, body) = match routes.iter().find(|(p, _)| *p == path) {
                    Some((_, b)) => ("200 OK", b.clone()),
                    None => ("404 Not Found", Vec::new()),
                };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    /// The whole chain the hub's UPDATE NOW runs, minus the window: download, checksum, unpack, swap.
    #[test]
    fn update_now_downloads_verifies_and_replaces_the_hub() {
        let root = std::env::temp_dir().join(format!("vf-hub-e2e-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("apps/word")).unwrap();
        fs::write(root.join("apps/word/word.exe"), b"MZ-installed-app").unwrap();
        let exe = root.join("voidflare.exe");
        fs::write(&exe, b"MZ-hub-0.2.0").unwrap();

        let zip = root.join("made.zip");
        zip_with(&zip, &[("voidflare.exe", b"MZ-hub-0.3.0")]);
        let zip_bytes = fs::read(&zip).unwrap();
        fs::remove_file(&zip).unwrap();
        let name = "voidflare-hub-0.3.0-windows.zip";
        let good = format!("{}  {name}\n", hex(Sha256::digest(&zip_bytes).as_slice()));
        let bad = format!("{}  {name}\n", "0".repeat(64));
        let base = serve(vec![
            (format!("/{name}"), zip_bytes.clone()),
            ("/good.txt".into(), good.into_bytes()),
            ("/bad.txt".into(), bad.into_bytes()),
        ]);
        let release = |sums: &str| Release {
            version: "0.3.0".into(),
            page: String::new(),
            notes: String::new(),
            zip: None,
            sums: Some(Asset { name: "SHA256SUMS.txt".into(), url: format!("{base}/{sums}"), size: 0 }),
            assets: vec![Asset { name: name.into(), url: format!("{base}/{name}"), size: zip_bytes.len() as u64 }],
        };
        let cancel = AtomicBool::new(false);

        // a checksum that does not match: nothing changes, nothing is left behind
        let err = install_hub_at(&exe, &release("bad.txt"), &|_, _| {}, &cancel).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
        assert_eq!(fs::read(&exe).unwrap(), b"MZ-hub-0.2.0");
        assert!(!root.join(STAGING).exists() && !root.join("voidflare.old.exe").exists());

        // the right one: the hub is replaced, the old one parked, the app and the staging folder untouched/gone
        let seen = std::cell::Cell::new(0u64);
        let back = install_hub_at(&exe, &release("good.txt"), &|d, _| seen.set(d), &cancel).expect("the update installs");
        assert_eq!(back, exe);
        assert_eq!(seen.get(), zip_bytes.len() as u64, "progress reached the end");
        assert_eq!(fs::read(&exe).unwrap(), b"MZ-hub-0.3.0");
        assert_eq!(fs::read(root.join("voidflare.old.exe")).unwrap(), b"MZ-hub-0.2.0");
        assert_eq!(fs::read(root.join("apps/word/word.exe")).unwrap(), b"MZ-installed-app");
        assert!(!root.join(STAGING).exists(), "the staging folder is removed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_update_changes_nothing() {
        let root = std::env::temp_dir().join(format!("vf-hub-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("voidflare.exe");
        fs::write(&exe, b"MZ-old-hub").unwrap();

        let no_exe = root.join("no-exe.zip");
        zip_with(&no_exe, &[("notes.txt", b"nothing useful")]);
        assert!(install_from_zip(&no_exe, &exe, &root.join(".w1")).is_err());
        let not_a_program = root.join("text.zip");
        zip_with(&not_a_program, &[("voidflare.exe", b"this is text, not an exe")]);
        assert!(install_from_zip(&not_a_program, &exe, &root.join(".w2")).unwrap_err().contains("usable"));
        let hostile = root.join("hostile.zip");
        zip_with(&hostile, &[("../evil.exe", b"MZ-evil")]);
        assert!(install_from_zip(&hostile, &exe, &root.join(".w3")).is_err());

        assert_eq!(fs::read(&exe).unwrap(), b"MZ-old-hub");
        assert!(!root.join("voidflare.old.exe").exists() && !root.join("..").join("evil.exe").exists());
        // a swap that cannot finish puts the old hub back
        assert!(replace_exe(&exe, &root.join("missing.exe")).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"MZ-old-hub");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sha256_matches_a_known_value() {
        assert_eq!(hex(Sha256::digest(b"abc").as_slice()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
