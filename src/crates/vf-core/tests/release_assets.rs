//! Checks the files `release.ps1` (Linux: `release.sh`) wrote into `..\release\` the way the hub will use them: the checksum file
//! covers every file, the manifest is readable, and every pack installs from its own ZIPs and runs its engine.
//! Skipped when there is no manifest for this platform in `release\` (CI and fresh checkouts have not run the script).
//!
//! Run it with:  cargo test --release -p vf-core --test release_assets -- --nocapture

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vf_core::engine;
use vf_core::packs::{self, EngineSource, Manifest};
use vf_core::update;

fn release_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("release")
}

fn load() -> Option<(PathBuf, String, Manifest)> {
    let dir = release_dir();
    let manifest_text = fs::read_to_string(dir.join(packs::MANIFEST_NAME)).ok()?;
    let sums = fs::read_to_string(dir.join("SHA256SUMS.txt")).expect("SHA256SUMS.txt next to the manifest");
    let manifest = packs::parse_manifest(&manifest_text).expect("the manifest parses");
    Some((dir, sums, manifest))
}

#[test]
fn every_release_file_matches_its_checksum() {
    let Some((dir, sums, _)) = load() else { return };
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "SHA256SUMS.txt" {
            continue;
        }
        let expected = update::checksum_for(&sums, &name).unwrap_or_else(|| panic!("SHA256SUMS.txt does not list {name}"));
        assert_eq!(packs::sha256_file(&entry.path()).unwrap(), expected, "{name}");
        checked += 1;
    }
    println!("{checked} files match SHA256SUMS.txt");
    assert!(checked >= 4);
}

#[test]
fn zips_use_forward_slashes_and_safe_names() {
    let Some((dir, _, _)) = load() else { return };
    for entry in fs::read_dir(&dir).unwrap().flatten() {
        if entry.path().extension().is_none_or(|e| e != "zip") {
            continue;
        }
        let mut archive = zip::ZipArchive::new(fs::File::open(entry.path()).unwrap()).unwrap();
        for i in 0..archive.len() {
            let file = archive.by_index(i).unwrap();
            let name = file.name().expect("a readable entry name").into_owned();
            assert!(!name.contains('\\'), "{}: backslash in {name:?}", entry.path().display());
            assert!(packs::safe_relative(&name).is_some(), "{}: unsafe entry {name:?}", entry.path().display());
        }
    }
}

#[test]
fn every_pack_installs_from_its_own_files_and_its_engine_runs() {
    let Some((dir, sums, manifest)) = load() else { return };
    let apps = std::env::temp_dir().join(format!("vf_release_assets_{}", std::process::id()));
    let _ = fs::remove_dir_all(&apps);
    let cancel = AtomicBool::new(false);
    println!("release {} with {} packs", manifest.version, manifest.packs.len());
    for pack in &manifest.packs {
        let app_zip = dir.join(&pack.app_asset);
        assert!(app_zip.is_file(), "{} is listed but missing", pack.app_asset);
        let app_hash = update::checksum_for(&sums, &pack.app_asset).expect("app zip is in the checksums");
        let engine_zip = pack.engine.as_ref().map(|e| dir.join(&e.asset));
        if let Some(z) = &engine_zip {
            assert!(z.is_file(), "{} is listed but missing", z.display());
            assert!(update::checksum_for(&sums, &pack.engine.as_ref().unwrap().asset).is_some());
        }
        let choice = pack.engine.as_ref().map(|_| (&EngineSource::Bundled, engine_zip.as_deref()));
        let st = packs::install_from_files(&apps, pack, &app_zip, &app_hash, choice, &cancel).unwrap_or_else(|e| panic!("{}: {e}", pack.id));
        assert!(apps.join(&pack.id).join(vf_core::platform::exe(&pack.id)).is_file());
        assert_eq!(packs::installed(&apps, &pack.id), Some(st));
        assert_eq!(packs::status(&apps, pack, &sums), packs::Status::UpToDate);
        assert_eq!(packs::health(&apps, pack, None), packs::Health::Ok, "{}", pack.id);
        if let Some(spec) = &pack.engine {
            let cli = apps.join(&pack.id).join(&spec.folder).join(&spec.cli);
            let got = engine::version_of(&cli).unwrap_or_else(|| panic!("{} did not answer --version", cli.display()));
            println!("{:<14} app + engine {} {}", pack.id, spec.name, got);
            assert_eq!(got, spec.version, "the bundled {} reports a different version than the manifest says", spec.name);
        }
    }
    let _ = fs::remove_dir_all(&apps);
}
