//! Packs: the hub stays small and each converter is its own download.
//!
//! A release carries a `manifest.json` that lists the packs, plus one ZIP per app and one per engine. The
//! hub installs a pack into `<hub folder>\apps\<id>\` (the folder it already looks in, so a pack installed
//! here is found exactly like one from the full bundle). Hashes are not repeated in the manifest: they come
//! from the release's `SHA256SUMS.txt`, one source of truth, and the manifest itself is listed there too.
//!
//! Everything in the manifest is untrusted text: ids, folder and program names become paths, so they are
//! checked to be plain names before they are used, and ZIP entries are checked one by one (see `safe_relative`).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::update::{self, Release};

/// The list of packs in a release. Windows keeps the original name, so hubs up to 0.5 still find it; each other
/// platform has its own, because its packs are other files.
pub const MANIFEST_NAME: &str = if cfg!(windows) { "manifest.json" } else { "manifest-linux-x86_64.json" };
const STATE_FILE: &str = "pack.json";
/// Upper bounds for one ZIP: far above any real pack (the largest is ~110 MB), far below a ZIP bomb.
const MAX_ENTRIES: usize = 20_000;
const MAX_UNPACKED: u64 = 2 * 1024 * 1024 * 1024;

/// How a pack gets the ArtCraft engine it drives.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineSpec {
    /// Lower-case engine name, e.g. `wordcraft`.
    pub name: String,
    /// The folder inside the pack that holds it, e.g. `wordcraft`.
    pub folder: String,
    /// The command-line program inside that folder, e.g. `wordcraft-cli.exe`.
    pub cli: String,
    /// The version we bundle and have tested.
    pub version: String,
    /// The engine ZIP in the release.
    pub asset: String,
    /// Installed engine versions we have tested this pack against (see `engine reuse`); empty = never reuse.
    pub compatible: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    /// e.g. `word-to-pdf`; also the folder name under `apps\`.
    pub id: String,
    pub name: String,
    pub version: String,
    /// The app ZIP in the release (the exe, its icons and notices; no engine).
    pub app_asset: String,
    pub engine: Option<EngineSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub schema: u32,
    /// The release version this manifest describes.
    pub version: String,
    pub packs: Vec<Pack>,
}

/// Where a pack's engine comes from once installed.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineSource {
    /// Our own trimmed copy, unpacked inside the pack.
    Bundled,
    /// The ArtCraft install already on this PC, at this version (nothing was downloaded).
    System { version: String },
}

/// What `pack.json` records about an installed pack.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub id: String,
    pub version: String,
    /// SHA-256 of the app ZIP that was installed: what tells an update apart even if the version text did not move.
    pub app_sha256: String,
    pub engine: Option<(EngineSource, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    NotInstalled,
    UpToDate,
    UpdateAvailable,
}

/// A plain file or folder name: no separators, no `..`, no drive or stream colon, no trailing dot or space.
pub fn is_plain_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && !s.ends_with(['.', ' '])
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn plain(value: &Value, key: &str) -> Result<String, String> {
    let s = value.get(key).and_then(Value::as_str).ok_or_else(|| format!("The pack list is missing `{key}`."))?;
    if !is_plain_name(s) {
        return Err(format!("The pack list has an unusable `{key}`: {s:?}."));
    }
    Ok(s.to_owned())
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 200)
        .map(str::to_owned)
        .ok_or_else(|| format!("The pack list is missing `{key}`."))
}

pub fn parse_manifest(json: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("The pack list could not be read: {e}"))?;
    let schema = v.get("schema").and_then(Value::as_u64).ok_or("The pack list has no schema number.")?;
    if schema != 1 {
        return Err(format!("This pack list is version {schema}; update voidflare to read it."));
    }
    let version = text(&v, "version")?;
    let mut packs = Vec::new();
    for p in v.get("packs").and_then(Value::as_array).ok_or("The pack list has no packs.")? {
        let engine = match p.get("engine") {
            None | Some(Value::Null) => None,
            Some(e) => Some(EngineSpec {
                name: plain(e, "name")?,
                folder: plain(e, "folder")?,
                cli: plain(e, "cli")?,
                version: text(e, "version")?,
                asset: plain(e, "asset")?,
                compatible: e
                    .get("compatible")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_default(),
            }),
        };
        let pack = Pack { id: plain(p, "id")?, name: text(p, "name")?, version: text(p, "version")?, app_asset: plain(p, "app_asset")?, engine };
        if packs.iter().any(|q: &Pack| q.id == pack.id) {
            return Err(format!("The pack list names `{}` twice.", pack.id));
        }
        packs.push(pack);
    }
    Ok(Manifest { schema: 1, version, packs })
}

/// Reads the manifest of a release, checking it against the release's `SHA256SUMS.txt` first.
pub fn fetch_manifest(release: &Release) -> Result<Manifest, String> {
    let asset = release.find_asset(MANIFEST_NAME).ok_or("This release has no pack list; use the full download instead.")?;
    let sums = update::sums_text(release)?;
    let expected = update::checksum_for(&sums, MANIFEST_NAME).ok_or("The checksum file does not list the pack list.")?;
    let body = update::fetch_text(&asset.url)?;
    if hex(Sha256::digest(body.as_bytes()).as_slice()) != expected {
        return Err("The pack list does not match its published checksum, so it was ignored.".to_owned());
    }
    parse_manifest(&body)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(h.finalize().as_slice()))
}

// ---------------------------------------------------------------- installed state

fn state_path(apps_dir: &Path, id: &str) -> PathBuf {
    apps_dir.join(id).join(STATE_FILE)
}

/// What is installed for this pack, if `pack.json` says so. A folder without it (the full bundle, an old
/// install) counts as not managed: the hub offers to install the pack but never deletes it unasked.
pub fn installed(apps_dir: &Path, id: &str) -> Option<Installed> {
    let v: Value = serde_json::from_str(&fs::read_to_string(state_path(apps_dir, id)).ok()?).ok()?;
    let engine = v.get("engine").filter(|e| !e.is_null()).and_then(|e| {
        let version = e.get("version")?.as_str()?.to_owned();
        let source = match e.get("source")?.as_str()? {
            "bundled" => EngineSource::Bundled,
            "system" => EngineSource::System { version: version.clone() },
            _ => return None,
        };
        Some((source, version))
    });
    Some(Installed {
        id: v.get("id")?.as_str()?.to_owned(),
        version: v.get("version")?.as_str()?.to_owned(),
        app_sha256: v.get("app_sha256")?.as_str()?.to_owned(),
        engine,
    })
}

fn write_state(dir: &Path, st: &Installed) -> Result<(), String> {
    let engine = st.engine.as_ref().map(|(source, version)| {
        json!({"source": match source { EngineSource::Bundled => "bundled", EngineSource::System { .. } => "system" }, "version": version})
    });
    let body = json!({"id": st.id, "version": st.version, "app_sha256": st.app_sha256, "engine": engine});
    fs::write(dir.join(STATE_FILE), serde_json::to_string_pretty(&body).unwrap_or_default() + "\n").map_err(|e| format!("Cannot write the install record: {e}"))
}

/// Whether the release has something newer than what is installed. `sums` is the release's `SHA256SUMS.txt`.
pub fn status(apps_dir: &Path, pack: &Pack, sums: &str) -> Status {
    let Some(have) = installed(apps_dir, &pack.id) else { return Status::NotInstalled };
    let newer_hash = update::checksum_for(sums, &pack.app_asset).is_some_and(|h| h != have.app_sha256);
    // Our own copy of the engine moved to another version (the PC's engine is not ours to update).
    let engine_moved = match (&have.engine, &pack.engine) {
        (Some((EngineSource::Bundled, v)), Some(spec)) => *v != spec.version,
        _ => false,
    };
    if update::is_newer(&pack.version, &have.version) || newer_hash || engine_moved { Status::UpdateAvailable } else { Status::UpToDate }
}

// ---------------------------------------------------------------- unpacking

/// A relative path from a ZIP entry name, or None if it could escape the folder or hide data: absolute or
/// drive paths, `..`, a colon (NTFS alternate streams), control characters, a part ending in `.` or a space.
pub fn safe_relative(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.starts_with(['/', '\\']) {
        return None;
    }
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() {
            continue; // a trailing slash or a doubled one
        }
        if part == "." || part == ".." || part.contains(':') || part.chars().any(char::is_control) || part.ends_with(['.', ' ']) {
            return None;
        }
        out.push(part);
    }
    (out.components().count() > 0).then_some(out)
}

/// Unpacks `zip_path` into `dest` (created if missing). Fails on the first unsafe entry, a symlink, too many
/// entries or too much data, and the caller removes the half-filled folder.
pub fn extract_zip(zip_path: &Path, dest: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("Cannot open {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{} is not a usable ZIP: {e}", zip_path.display()))?;
    if archive.len() > MAX_ENTRIES {
        return Err("The ZIP has too many files.".to_owned());
    }
    fs::create_dir_all(dest).map_err(|e| format!("Cannot create {}: {e}", dest.display()))?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled.".to_owned());
        }
        let mut entry = archive.by_index(i).map_err(|e| format!("A file in the ZIP could not be read: {e}"))?;
        let name = entry.name().map_err(|e| format!("A file name in the ZIP could not be read ({e}); nothing was installed."))?.into_owned();
        let rel = safe_relative(&name).ok_or_else(|| format!("The ZIP holds an unsafe path ({name:?}); nothing was installed."))?;
        if entry.is_symlink() {
            return Err(format!("The ZIP holds a link ({name:?}); nothing was installed."));
        }
        let target = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| format!("Cannot create {}: {e}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
        }
        let mut out = fs::File::create(&target).map_err(|e| format!("Cannot write {}: {e}", target.display()))?;
        // Count what is really written, not what the entry claims.
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(|e| format!("A file in the ZIP could not be unpacked: {e}"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_UNPACKED {
                return Err("The ZIP unpacks to far more data than expected; nothing was installed.".to_owned());
            }
            out.write_all(&buf[..n]).map_err(|e| format!("Cannot write {}: {e}", target.display()))?;
        }
        // Linux: keep the permissions the file was packed with, or the programs inside would not run.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            drop(out);
            crate::platform::set_mode(&target, mode);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- engine reuse

/// Where a pack's engine should come from, given the version of the ArtCraft engine found on this PC (if any).
/// The PC's engine is reused only when its exact version is one we have tested this pack against
/// (`compatible`); anything else, including a newer one nobody has tried, gets our own tested copy. A pack
/// with an empty list (CADCraft: the official build still draws tilted blocks wrongly) never reuses.
pub fn choose_engine(spec: &EngineSpec, system_version: Option<&str>) -> EngineSource {
    match system_version {
        Some(v) if spec.compatible.iter().any(|c| c == v) => EngineSource::System { version: v.to_owned() },
        _ => EngineSource::Bundled,
    }
}

/// How an installed pack stands with its engine, checked each time the hub starts (ArtCraft can update or
/// be uninstalled after the pack was installed).
#[derive(Clone, Debug, PartialEq)]
pub enum Health {
    NotInstalled,
    Ok,
    /// The engine the pack relies on is gone: reinstall the pack to get our own copy.
    EngineMissing,
    /// ArtCraft updated its engine to a version we have not tested: the pack keeps working at the user's risk
    /// until a release lists that version, or until the user installs our own copy.
    EngineUntested { found: String },
}

pub fn health(apps_dir: &Path, pack: &Pack, system_version: Option<&str>) -> Health {
    let Some(have) = installed(apps_dir, &pack.id) else { return Health::NotInstalled };
    let Some(spec) = &pack.engine else { return Health::Ok };
    match have.engine {
        Some((EngineSource::Bundled, _)) => {
            if apps_dir.join(&pack.id).join(&spec.folder).join(&spec.cli).is_file() { Health::Ok } else { Health::EngineMissing }
        }
        Some((EngineSource::System { .. }, _)) => match system_version {
            None => Health::EngineMissing,
            Some(v) if spec.compatible.iter().any(|c| c == v) => Health::Ok,
            Some(v) => Health::EngineUntested { found: v.to_owned() },
        },
        None => Health::EngineMissing,
    }
}

/// The version of the ArtCraft engine installed on this PC for this pack, if there is one (runs `--version`).
pub fn system_engine_version(spec: &EngineSpec) -> Option<String> {
    let e = crate::engine::by_name(&spec.name)?;
    crate::engine::version_of(&e.find_system()?)
}

/// What installing a pack would do, for the card to say before the user clicks.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// None when the pack drives no engine.
    pub engine: Option<EngineSource>,
    /// What has to be downloaded: the app ZIP, plus our engine ZIP unless the PC's engine is reused.
    pub download_bytes: u64,
}

pub fn plan(release: &Release, pack: &Pack, system_version: Option<&str>) -> Plan {
    let size = |name: &str| release.find_asset(name).map_or(0, |a| a.size);
    let engine = pack.engine.as_ref().map(|spec| choose_engine(spec, system_version));
    let engine_bytes = match (&pack.engine, &engine) {
        (Some(spec), Some(EngineSource::Bundled)) => size(&spec.asset),
        _ => 0,
    };
    Plan { engine, download_bytes: size(&pack.app_asset) + engine_bytes }
}

// ---------------------------------------------------------------- install and remove

fn remove_dir_quiet(p: &Path) {
    let _ = fs::remove_dir_all(p);
}

/// Installs a pack from ZIPs that are already on disk and verified: the app ZIP, and the engine ZIP when the
/// engine is `Bundled`. Everything is unpacked beside the final folder and swapped in by renames, so a failure
/// or a cancel leaves the previous install untouched.
pub fn install_from_files(
    apps_dir: &Path,
    pack: &Pack,
    app_zip: &Path,
    app_sha256: &str,
    engine: Option<(&EngineSource, Option<&Path>)>,
    cancel: &AtomicBool,
) -> Result<Installed, String> {
    let final_dir = apps_dir.join(&pack.id);
    let staging = apps_dir.join(format!(".{}.installing", pack.id));
    let previous = apps_dir.join(format!(".{}.previous", pack.id));
    fs::create_dir_all(apps_dir).map_err(|e| format!("Cannot create {}: {e}", apps_dir.display()))?;
    remove_dir_quiet(&staging);
    remove_dir_quiet(&previous);

    let built = (|| -> Result<Installed, String> {
        extract_zip(app_zip, &staging, cancel)?;
        let exe_name = crate::platform::exe(&pack.id);
        let exe = staging.join(&exe_name);
        if !exe.is_file() {
            return Err(format!("The app ZIP does not contain {exe_name}, so it was not installed."));
        }
        let mut recorded = None;
        if let (Some(spec), Some((source, zip))) = (&pack.engine, engine) {
            if let EngineSource::Bundled = source {
                let zip = zip.ok_or("The engine ZIP is missing.")?;
                let engine_dir = staging.join(&spec.folder);
                extract_zip(zip, &engine_dir, cancel)?;
                if !engine_dir.join(&spec.cli).is_file() {
                    return Err(format!("The engine ZIP does not contain {}, so it was not installed.", spec.cli));
                }
                recorded = Some((EngineSource::Bundled, spec.version.clone()));
            } else if let EngineSource::System { version } = source {
                recorded = Some((EngineSource::System { version: version.clone() }, version.clone()));
            }
        }
        let st = Installed { id: pack.id.clone(), version: pack.version.clone(), app_sha256: app_sha256.to_owned(), engine: recorded };
        write_state(&staging, &st)?;
        Ok(st)
    })();

    let st = match built {
        Ok(st) => st,
        Err(e) => {
            remove_dir_quiet(&staging);
            return Err(e);
        }
    };
    // Swap: old folder aside, new folder in, old folder gone. If the first rename fails the app is running.
    if final_dir.exists() {
        if let Err(e) = fs::rename(&final_dir, &previous) {
            remove_dir_quiet(&staging);
            return Err(format!("Cannot replace {}: {e}. Close the app and try again.", pack.name));
        }
    }
    if let Err(e) = fs::rename(&staging, &final_dir) {
        if previous.exists() {
            let _ = fs::rename(&previous, &final_dir); // put the old one back
        }
        remove_dir_quiet(&staging);
        return Err(format!("Cannot finish installing {}: {e}", pack.name));
    }
    remove_dir_quiet(&previous);
    Ok(st)
}

/// Removes an installed pack, but only one this installer made (it has `pack.json`).
pub fn remove(apps_dir: &Path, id: &str) -> Result<(), String> {
    if !is_plain_name(id) {
        return Err("Unusable pack name.".to_owned());
    }
    let dir = apps_dir.join(id);
    if !state_path(apps_dir, id).is_file() {
        return Err("This app was not installed by voidflare's installer, so it is left alone.".to_owned());
    }
    let gone = apps_dir.join(format!(".{id}.removing"));
    remove_dir_quiet(&gone);
    fs::rename(&dir, &gone).map_err(|e| format!("Cannot remove it: {e}. Close the app and try again."))?;
    remove_dir_quiet(&gone);
    Ok(())
}

/// Downloads and installs one pack from a release. `engine` says where its engine comes from (the caller
/// decides, see `engine reuse`); `progress` gets (stage, bytes so far, total).
pub fn install(
    apps_dir: &Path,
    release: &Release,
    sums: &str,
    pack: &Pack,
    engine: Option<&EngineSource>,
    progress: &dyn Fn(&str, u64, u64),
    cancel: &AtomicBool,
) -> Result<Installed, String> {
    let scratch = std::env::temp_dir().join(format!("voidflare-pack-{}", pack.id));
    let _ = fs::remove_dir_all(&scratch);
    let result = (|| -> Result<Installed, String> {
        let app_asset = release.find_asset(&pack.app_asset).ok_or_else(|| format!("The release has no {}.", pack.app_asset))?;
        let app_hash = update::checksum_for(sums, &app_asset.name).ok_or_else(|| format!("The checksum file does not list {}.", app_asset.name))?;
        let app_zip = update::fetch_verified(app_asset, &app_hash, &scratch, &|d, t| progress("app", d, t), cancel)?;

        let mut engine_zip = None;
        if let (Some(spec), Some(EngineSource::Bundled)) = (&pack.engine, engine) {
            let asset = release.find_asset(&spec.asset).ok_or_else(|| format!("The release has no {}.", spec.asset))?;
            let hash = update::checksum_for(sums, &asset.name).ok_or_else(|| format!("The checksum file does not list {}.", asset.name))?;
            engine_zip = Some(update::fetch_verified(asset, &hash, &scratch, &|d, t| progress("engine", d, t), cancel)?);
        }
        let choice = engine.map(|source| (source, engine_zip.as_deref()));
        install_from_files(apps_dir, pack, &app_zip, &app_hash, choice, cancel)
    })();
    let _ = fs::remove_dir_all(&scratch);
    result
}

#[cfg(test)]
mod tests {
    /// The app program inside a pack: `word-to-pdf.exe` on Windows, `word-to-pdf` on Linux.
    const APP_EXE: &str = if cfg!(windows) { "word-to-pdf.exe" } else { "word-to-pdf" };
    use super::*;

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vf-packs-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn pack() -> Pack {
        Pack {
            id: "word-to-pdf".into(),
            name: "Word to PDF".into(),
            version: "0.2.0".into(),
            app_asset: "app-word-to-pdf-0.2.0-windows-x64.zip".into(),
            engine: Some(EngineSpec {
                name: "wordcraft".into(),
                folder: "wordcraft".into(),
                cli: "wordcraft-cli.exe".into(),
                version: "0.12.0".into(),
                asset: "engine-wordcraft-0.12.0-windows-x64.zip".into(),
                compatible: vec!["0.12.0".into()],
            }),
        }
    }

    const MANIFEST: &str = r#"{
      "schema": 1, "version": "0.2.0",
      "packs": [{
        "id": "word-to-pdf", "name": "Word to PDF", "version": "0.2.0",
        "app_asset": "app-word-to-pdf-0.2.0-windows-x64.zip",
        "engine": {"name": "wordcraft", "folder": "wordcraft", "cli": "wordcraft-cli.exe", "version": "0.12.0",
                   "asset": "engine-wordcraft-0.12.0-windows-x64.zip", "compatible": ["0.12.0"]}
      }]
    }"#;

    #[test]
    fn a_manifest_is_read() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.packs, vec![pack()]);
    }

    #[test]
    fn a_manifest_with_paths_in_names_is_refused() {
        for bad in ["../x", "a/b", "a\\b", "C:x", "x.", "", " ", "x y"] {
            let json = MANIFEST.replace("\"word-to-pdf\"", &format!("{bad:?}"));
            assert!(parse_manifest(&json).is_err(), "{bad:?} should be refused");
        }
        let json = MANIFEST.replace("wordcraft-cli.exe", "..\\evil.exe");
        assert!(parse_manifest(&json).is_err());
        assert!(parse_manifest(&MANIFEST.replace("\"schema\": 1", "\"schema\": 2")).is_err());
        assert!(parse_manifest("{}").is_err());
        assert!(parse_manifest("nope").is_err());
        let twice = MANIFEST.replace("\"packs\": [{", "\"packs\": [{\"id\":\"word-to-pdf\",\"name\":\"x\",\"version\":\"1\",\"app_asset\":\"a.zip\"},{");
        assert!(parse_manifest(&twice).is_err());
    }

    #[test]
    fn unsafe_zip_paths_are_refused() {
        for bad in ["../evil.exe", "..\\evil.exe", "/abs.exe", "\\abs.exe", "C:/x.exe", "a/../../b", "file.txt:stream", "dir./f", "bad\u{1}name", ""] {
            assert!(safe_relative(bad).is_none(), "{bad:?} should be refused");
        }
        assert_eq!(safe_relative("a/b/c.txt"), Some(PathBuf::from("a").join("b").join("c.txt")));
        assert_eq!(safe_relative("a\\b.txt"), Some(PathBuf::from("a").join("b.txt")));
        assert_eq!(safe_relative("dir/"), Some(PathBuf::from("dir")));
    }

    #[test]
    fn a_zip_that_escapes_installs_nothing() {
        let d = scratch("escape");
        let z = d.join("evil.zip");
        zip_with(&z, &[("ok.txt", b"fine"), ("../escaped.txt", b"bad")]);
        let dest = d.join("out");
        let err = extract_zip(&z, &dest, &AtomicBool::new(false)).unwrap_err();
        assert!(err.contains("unsafe"), "{err}");
        assert!(!d.join("escaped.txt").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pack_installs_updates_and_is_removed() {
        let d = scratch("flow");
        let apps = d.join("apps");
        let (app_zip, eng_zip) = (d.join("app.zip"), d.join("engine.zip"));
        zip_with(&app_zip, &[(APP_EXE, b"exe v1"), ("LICENSE-MIT", b"mit")]);
        zip_with(&eng_zip, &[("wordcraft-cli.exe", b"engine"), ("models/m.bin", b"model")]);
        let cancel = AtomicBool::new(false);
        let p = pack();
        let (h1, h2) = ("aa".repeat(32), "bb".repeat(32));

        let st = install_from_files(&apps, &p, &app_zip, &h1, Some((&EngineSource::Bundled, Some(&eng_zip))), &cancel).unwrap();
        assert_eq!(st.engine, Some((EngineSource::Bundled, "0.12.0".into())));
        assert_eq!(fs::read(apps.join("word-to-pdf").join(APP_EXE)).unwrap(), b"exe v1");
        assert_eq!(fs::read(apps.join("word-to-pdf/wordcraft/models/m.bin")).unwrap(), b"model");
        assert_eq!(installed(&apps, "word-to-pdf"), Some(st));
        assert!(!apps.join(".word-to-pdf.installing").exists() && !apps.join(".word-to-pdf.previous").exists());

        // Status: same hash and version is current; a different published hash is an update.
        let sums = format!("{h2}  {}\n", p.app_asset);
        assert_eq!(status(&apps, &p, &sums), Status::UpdateAvailable, "the published hash differs from the installed one");
        let same = format!("{h1}  {}\n", p.app_asset);
        assert_eq!(status(&apps, &p, &same), Status::UpToDate);
        // A newer version number alone is also an update, even with the same hash.
        let newer = Pack { version: "0.3.0".into(), ..p.clone() };
        assert_eq!(status(&apps, &newer, &same), Status::UpdateAvailable);
        assert_eq!(status(&apps, &Pack { id: "other".into(), ..p.clone() }, &sums), Status::NotInstalled);

        // Update to a new app ZIP, engine reused from the PC: the bundled engine folder goes away with the old install.
        zip_with(&app_zip, &[(APP_EXE, b"exe v2")]);
        let sys = EngineSource::System { version: "0.13.2".into() };
        let st2 = install_from_files(&apps, &p, &app_zip, &h2, Some((&sys, None)), &cancel).unwrap();
        assert_eq!(st2.engine, Some((sys.clone(), "0.13.2".into())));
        assert_eq!(fs::read(apps.join("word-to-pdf").join(APP_EXE)).unwrap(), b"exe v2");
        assert!(!apps.join("word-to-pdf/wordcraft").exists(), "no duplicate engine when the PC's is used");
        assert_eq!(installed(&apps, "word-to-pdf").unwrap().engine, Some((sys, "0.13.2".into())));

        remove(&apps, "word-to-pdf").unwrap();
        assert!(!apps.join("word-to-pdf").exists());
        assert!(remove(&apps, "word-to-pdf").is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_install_keeps_the_previous_one() {
        let d = scratch("keep");
        let apps = d.join("apps");
        let (good, bad) = (d.join("good.zip"), d.join("bad.zip"));
        zip_with(&good, &[(APP_EXE, b"works")]);
        zip_with(&bad, &[("readme.txt", b"no exe in here")]);
        let cancel = AtomicBool::new(false);
        let p = Pack { engine: None, ..pack() };
        install_from_files(&apps, &p, &good, "aa", None, &cancel).unwrap();
        let err = install_from_files(&apps, &p, &bad, "bb", None, &cancel).unwrap_err();
        assert!(err.contains("does not contain"), "{err}");
        assert_eq!(fs::read(apps.join("word-to-pdf").join(APP_EXE)).unwrap(), b"works");
        assert!(!apps.join(".word-to-pdf.installing").exists());
        // A folder that voidflare did not install (no pack.json) is never deleted.
        fs::create_dir_all(apps.join("foreign")).unwrap();
        assert!(remove(&apps, "foreign").is_err() && apps.join("foreign").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_cancelled_install_leaves_nothing() {
        let d = scratch("cancel");
        let apps = d.join("apps");
        let z = d.join("a.zip");
        zip_with(&z, &[(APP_EXE, b"x")]);
        let p = Pack { engine: None, ..pack() };
        let err = install_from_files(&apps, &p, &z, "aa", None, &AtomicBool::new(true)).unwrap_err();
        assert!(err.contains("Cancelled"), "{err}");
        assert!(!apps.join("word-to-pdf").exists() && !apps.join(".word-to-pdf.installing").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_pcs_engine_is_reused_only_when_tested() {
        let mut spec = pack().engine.unwrap();
        spec.compatible = vec!["0.12.0".into(), "0.13.2".into()];
        assert_eq!(choose_engine(&spec, Some("0.13.2")), EngineSource::System { version: "0.13.2".into() });
        assert_eq!(choose_engine(&spec, Some("0.12.0")), EngineSource::System { version: "0.12.0".into() });
        assert_eq!(choose_engine(&spec, Some("0.14.0")), EngineSource::Bundled, "newer but never tried");
        assert_eq!(choose_engine(&spec, Some("0.13")), EngineSource::Bundled, "no prefix matching");
        assert_eq!(choose_engine(&spec, None), EngineSource::Bundled, "no engine on the PC");
        spec.compatible.clear();
        assert_eq!(choose_engine(&spec, Some("0.13.2")), EngineSource::Bundled, "an empty list never reuses");
    }

    #[test]
    fn the_plan_counts_only_what_has_to_be_downloaded() {
        let asset = |name: &str, size: u64| update::Asset { name: name.into(), url: String::new(), size };
        let release = Release {
            version: "0.2.0".into(),
            page: String::new(),
            notes: String::new(),
            zip: None,
            sums: None,
            assets: vec![asset("app-word-to-pdf-0.2.0-windows-x64.zip", 6_000_000), asset("engine-wordcraft-0.12.0-windows-x64.zip", 44_000_000)],
        };
        let mut p = pack();
        p.engine.as_mut().unwrap().compatible = vec!["0.13.2".into()];
        let own = plan(&release, &p, Some("0.14.0"));
        assert_eq!((own.engine, own.download_bytes), (Some(EngineSource::Bundled), 50_000_000));
        let reuse = plan(&release, &p, Some("0.13.2"));
        assert_eq!((reuse.engine, reuse.download_bytes), (Some(EngineSource::System { version: "0.13.2".into() }), 6_000_000));
        let none = plan(&release, &Pack { engine: None, ..p }, Some("0.13.2"));
        assert_eq!((none.engine, none.download_bytes), (None, 6_000_000));
    }

    #[test]
    fn a_new_bundled_engine_version_is_an_update_but_the_pcs_engine_is_not_ours_to_update() {
        let d = scratch("engmove");
        let apps = d.join("apps");
        let (app_zip, eng_zip) = (d.join("app.zip"), d.join("engine.zip"));
        zip_with(&app_zip, &[(APP_EXE, b"x")]);
        zip_with(&eng_zip, &[("wordcraft-cli.exe", b"e")]);
        let h = "dd".repeat(32);
        let p = pack();
        let same = format!("{h}  {}
", p.app_asset);
        let cancel = AtomicBool::new(false);
        install_from_files(&apps, &p, &app_zip, &h, Some((&EngineSource::Bundled, Some(&eng_zip))), &cancel).unwrap();
        assert_eq!(status(&apps, &p, &same), Status::UpToDate);
        let mut moved = p.clone();
        moved.engine.as_mut().unwrap().version = "0.13.0".into();
        assert_eq!(status(&apps, &moved, &same), Status::UpdateAvailable);
        let sys = EngineSource::System { version: "0.13.2".into() };
        install_from_files(&apps, &p, &app_zip, &h, Some((&sys, None)), &cancel).unwrap();
        assert_eq!(status(&apps, &moved, &same), Status::UpToDate);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn health_follows_the_engine() {
        let d = scratch("health");
        let apps = d.join("apps");
        let (app_zip, eng_zip) = (d.join("app.zip"), d.join("engine.zip"));
        zip_with(&app_zip, &[(APP_EXE, b"x")]);
        zip_with(&eng_zip, &[("wordcraft-cli.exe", b"e")]);
        let mut p = pack();
        p.engine.as_mut().unwrap().compatible = vec!["0.13.2".into()];
        let cancel = AtomicBool::new(false);
        let h = "cc".repeat(32);
        assert_eq!(health(&apps, &p, Some("0.13.2")), Health::NotInstalled);

        // Bundled: healthy while its files are there, regardless of what ArtCraft does.
        install_from_files(&apps, &p, &app_zip, &h, Some((&EngineSource::Bundled, Some(&eng_zip))), &cancel).unwrap();
        assert_eq!(health(&apps, &p, None), Health::Ok);
        fs::remove_file(apps.join("word-to-pdf/wordcraft/wordcraft-cli.exe")).unwrap();
        assert_eq!(health(&apps, &p, None), Health::EngineMissing);

        // System: follows the PC's engine.
        let sys = EngineSource::System { version: "0.13.2".into() };
        install_from_files(&apps, &p, &app_zip, &h, Some((&sys, None)), &cancel).unwrap();
        assert_eq!(health(&apps, &p, Some("0.13.2")), Health::Ok);
        assert_eq!(health(&apps, &p, None), Health::EngineMissing, "ArtCraft was uninstalled");
        assert_eq!(health(&apps, &p, Some("0.14.0")), Health::EngineUntested { found: "0.14.0".into() }, "ArtCraft updated");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn plain_names() {
        assert!(is_plain_name("word-to-pdf") && is_plain_name("wordcraft-cli.exe") && is_plain_name("a_b.1"));
        assert!(!is_plain_name("") && !is_plain_name("..") && !is_plain_name("a/b") && !is_plain_name("a b") && !is_plain_name("é"));
    }
}
