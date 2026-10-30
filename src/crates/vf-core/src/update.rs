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

pub const REPO: &str = "Flaeren/voidflare";
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
        zip: assets.iter().find(|a| a.name.to_lowercase().ends_with(".zip")).cloned(),
        sums: assets.iter().find(|a| a.name == SUMS_NAME).cloned(),
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
    let body = ureq::get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
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
    let zip = release.zip.as_ref().ok_or("This release has no ZIP to download.")?;
    let sums = release.sums.as_ref().ok_or("This release has no checksum file, so it cannot be verified.")?;

    let sums_text = ureq::get(&sums.url)
        .header("User-Agent", user_agent())
        .call()
        .map_err(http_error)?
        .body_mut()
        .read_to_string()
        .map_err(http_error)?;
    let expected = checksum_for(&sums_text, &zip.name).ok_or_else(|| format!("{SUMS_NAME} does not list {}.", zip.name))?;

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
        if actual != expected {
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

    #[test]
    fn sha256_matches_a_known_value() {
        assert_eq!(hex(Sha256::digest(b"abc").as_slice()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
