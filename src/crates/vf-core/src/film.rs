//! Video and audio-from-video conversion on the FilmCraft engine.
//!
//! FilmCraft is an editor, not a converter: it has no `convert <in> <out>`. A conversion is three of its commands
//! run as a script on a throwaway project: import the file, make a sequence that matches it, export the sequence
//! in the wanted format. This module writes that script, runs it and delivers the result, so a video looks to the
//! apps like any other file (`Spec` in, new file out).
//!
//! `Spec::extra_args` carries `key=value` pairs that become parameters of FilmCraft's `file.exportMedia`
//! (`format=h264`, `bitrateMode=crf`, `crf=23`, `scale=0.5`, `audio=false`...). A value is read as JSON when it is
//! valid JSON (`0.5`, `false`) and as text otherwise.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::job::{self, Spec};

/// A long video can take a long time; this only stops a hung engine.
const LIMIT: Duration = Duration::from_secs(4 * 3600);

/// The `key=value` pairs of `args` as export parameters. Anything that is not `key=value` is an error.
pub fn export_params(args: &[String]) -> Result<Map<String, Value>, String> {
    let mut map = Map::new();
    for a in args {
        let (k, v) = a.split_once('=').filter(|(k, _)| !k.is_empty()).ok_or_else(|| format!("'{a}' is not a key=value export setting"))?;
        let value = serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_owned()));
        map.insert(k.to_owned(), value);
    }
    Ok(map)
}

/// The three-line script: import, sequence from the imported item, export and wait.
pub fn script(src: &Path, target: &Path, params: Map<String, Value>) -> String {
    let mut export = params;
    export.insert("path".into(), json!(target.to_string_lossy()));
    export.insert("wait".into(), json!(true));
    let lines = [
        json!({ "id": "file.import", "params": { "paths": [src.to_string_lossy()] } }),
        // The first item of a new project is 1; the sequence takes the clip's frame size and rate.
        json!({ "id": "file.newSequence", "params": { "name": "voidflare", "fromItem": 1 } }),
        json!({ "id": "file.exportMedia", "params": Value::Object(export) }),
    ];
    lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n"
}

/// What FilmCraft said when a line of the script failed (`{"ok":false,"error":"..."}`), if anything.
fn script_error(stdout: &str) -> Option<String> {
    stdout.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["ok"] == false).and_then(|v| v["error"].as_str().map(str::to_owned))
}

/// FilmCraft's own `probe` of the file, as JSON; `None` when it says nothing usable.
fn probe(cli: &Path, src: &Path) -> Option<Value> {
    let tmp = job::scratch_dir().ok()?;
    let mut cmd = Command::new(cli);
    cmd.arg("probe").arg(src);
    let out = job::run_capture(&mut cmd, &tmp).ok();
    let _ = fs::remove_dir_all(&tmp);
    serde_json::from_str(&out?).ok()
}

/// Whether the probed file has an audio stream; `None` when the probe says nothing usable.
fn has_audio(probe: &Value) -> Option<bool> {
    Some(!probe["audio_streams"].as_array()?.is_empty())
}

/// The codec of a video FilmCraft has no decoder for. It imports such a file without a word and then exports a card
/// that says "Media unreadable" in place of the pictures (a 21 MB old-codec AVI came out as a 168 KB MP4 of that
/// card), so the conversion must not go on. The probe names a codec it knows in words ("H.264 (High)", "Uncompressed
/// RGB 24-bit") and gives its video a pixel format; a codec it does not know is named by its bare four-letter code in
/// the file (`CRAM`) and has no pixel format. (An empty pixel format alone is not enough: an uncompressed AVI has none
/// and reads fine.) `None`: no video, or one it can read.
fn undecodable_codec(probe: &Value) -> Option<String> {
    let video = probe.get("video").filter(|v| v.is_object())?;
    if !video["pixel_format"].as_str().unwrap_or("").is_empty() {
        return None;
    }
    let codec = video["codec"].as_str()?.trim();
    (codec.len() == 4 && codec.chars().all(|c| c.is_ascii_alphanumeric())).then(|| codec.to_owned())
}

/// The most disk an uncompressed copy of an old-codec video may take while it is converted.
const MAX_RAW_BYTES: u64 = 8 << 30;

/// Output types that carry only sound.
const AUDIO_ONLY: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "aiff"];

/// Converts one video (or takes the sound of it) into `spec.out_ext`, into `out_dir`.
pub fn convert(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let params = export_params(&spec.extra_args)?;
    let probed = probe(&spec.cli, src);
    let audio_only = AUDIO_ONLY.contains(&spec.out_ext.as_str());
    // FilmCraft would write a silent file for a clip with no sound; say so instead.
    if audio_only && probed.as_ref().and_then(has_audio) == Some(false) {
        // Translated in the apps; the command line and the MCP server load no catalog, so they get the English.
        let said = crate::t("film.no_sound");
        return Err(if said == "film.no_sound" { "This video has no sound to take.".to_owned() } else { said });
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("out").to_owned();
    let tmp = job::scratch_dir().map_err(|e| e.to_string())?;
    let result = match probed.as_ref().and_then(undecodable_codec) {
        // Microsoft Video 1: FilmCraft cannot read it, so it is decoded here into an uncompressed AVI that it can
        Some(codec) if crate::msvideo1::is_msvideo1_name(&codec) => decode_old_avi(&tmp, src).and_then(|raw| run_in(&tmp, spec, &raw, out_dir, &stem, params)),
        // any other codec it does not know would come out as a "Media unreadable" card: refuse, and say which codec
        Some(codec) => {
            // Translated in the apps; the command line and the MCP server load no catalog, so they get the English.
            let said = crate::tf("film.unreadable", &[("codec", &codec)]);
            Err(if said == "film.unreadable" { format!("FilmCraft cannot read this video: its codec ({codec}) is not supported, so the result would only be a \"Media unreadable\" picture. Nothing was converted.") } else { said })
        }
        None => run_in(&tmp, spec, src, out_dir, &stem, params),
    };
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// Decodes an AVI made with Microsoft Video 1 into an uncompressed one in `tmp` (the pictures, and the sound when it is
/// plain PCM). The file is as big as the pictures are, so a very long video is refused rather than filling the disk.
fn decode_old_avi(tmp: &Path, src: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    if crate::msvideo1::raw_size(&bytes).is_some_and(|n| n > MAX_RAW_BYTES) {
        return Err("This video is too long to be converted: decoding its old codec would need more than 8 GB of disk space.".to_owned());
    }
    drop(bytes);
    let raw = tmp.join("decoded.avi");
    let stopped = || job::cancel_flag().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed));
    crate::msvideo1::to_raw_avi(src, &raw, &stopped)?;
    Ok(raw)
}

fn run_in(tmp: &Path, spec: &Spec, src: &Path, out_dir: &Path, stem: &str, params: Map<String, Value>) -> Result<PathBuf, String> {
    let target = tmp.join(format!("out.{}", spec.out_ext));
    let script_file = tmp.join("convert.jsonl");
    fs::write(&script_file, script(src, &target, params)).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&spec.cli);
    cmd.arg("run").arg(&script_file);
    if let Err(generic) = job::run_command_for(&mut cmd, tmp, LIMIT) {
        // The engine reports what went wrong on stdout, as JSON; that reads better than the last raw line.
        let said = fs::read_to_string(tmp.join("stdout.txt")).ok().and_then(|o| script_error(&o));
        return Err(said.unwrap_or(generic));
    }
    if !target.is_file() {
        return Err("the engine finished but wrote no file".to_owned());
    }
    job::deliver(&target, out_dir, &format!("{stem}.{}", spec.out_ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_become_export_parameters() {
        let m = export_params(&["format=h264".into(), "crf=23".into(), "scale=0.5".into(), "audio=false".into()]).unwrap();
        assert_eq!(m["format"], "h264");
        assert_eq!(m["crf"], 23);
        assert_eq!(m["scale"], 0.5);
        assert_eq!(m["audio"], false);
        assert!(export_params(&["nonsense".into()]).is_err());
        assert!(export_params(&["=x".into()]).is_err());
    }

    #[test]
    fn the_script_escapes_odd_paths() {
        let s = script(Path::new("C:\\Video\\Gita \u{e0} \u{301} \"x\".mp4"), Path::new(r"C:\t\out.mp4"), export_params(&["format=h264".into()]).unwrap());
        let lines: Vec<Value> = s.lines().map(|l| serde_json::from_str(l).expect("every line is valid JSON")).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["id"], "file.import");
        assert_eq!(lines[2]["params"]["wait"], true);
        assert_eq!(lines[2]["params"]["format"], "h264");
    }

    /// Real FilmCraft (skipped when it is not installed or bundled): a tiny demo clip is made with the engine
    /// itself, then converted to H.264, ProRes and GIF; each result is probed back; asking for the sound of a
    /// clip that has none is an error that says so.
    #[test]
    fn real_engine_converts_a_video() {
        let Some(cli) = crate::engine::FILMCRAFT.find() else { return };
        let dir = std::env::temp_dir().join(format!("vf_film_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Gita al mare \u{e0}.mp4");
        let made = Command::new(&cli)
            .args(["--demo", "export"])
            .arg(&src)
            .args(["--format", "h264", "--scale", "0.1"])
            .output()
            .expect("run the engine");
        assert!(made.status.success() && src.is_file(), "could not make the demo clip");
        let silent = dir.join("senza audio.mp4");
        let made = Command::new(&cli).args(["--demo", "export"]).arg(&silent).args(["--format", "h264", "--scale", "0.1", "--no-audio"]).output().expect("run the engine");
        assert!(made.status.success() && silent.is_file(), "could not make the silent clip");
        let probe = |p: &Path| -> Value {
            let o = Command::new(&cli).arg("probe").arg(p).output().expect("probe");
            serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("probe of {} printed no JSON", p.display()))
        };
        let (w0, h0) = (probe(&src)["video"]["width"].as_u64().unwrap(), probe(&src)["video"]["height"].as_u64().unwrap());
        let spec = |ext: &str, args: &[&str]| Spec { cli: cli.clone(), out_ext: ext.into(), extra_args: args.iter().map(|a| (*a).to_owned()).collect(), env: vec![] };

        let mp4 = convert(&spec("mp4", &["format=h264", "bitrateMode=crf", "crf=28", "scale=0.5"]), &src, &dir.join("o")).expect("h264");
        let v = probe(&mp4);
        assert!(v["video"]["codec"].as_str().unwrap().starts_with("H.264"), "{v}");
        assert!(v["video"]["width"].as_u64().unwrap() < w0 && v["video"]["height"].as_u64().unwrap() < h0, "half size is smaller: {v}");

        let mov = convert(&spec("mov", &["format=prores"]), &src, &dir.join("o")).expect("prores");
        assert!(probe(&mov)["video"]["codec"].as_str().unwrap().contains("ProRes"));

        let gif = convert(&spec("gif", &["format=gif", "audio=false"]), &src, &dir.join("o")).expect("gif");
        assert!(fs::metadata(&gif).unwrap().len() > 100);

        for (ext, format) in [("wav", "wav"), ("mp3", "mp3"), ("m4a", "aac"), ("flac", "flac")] {
            let audio = convert(&spec(ext, &[&format!("format={format}")]), &src, &dir.join("o")).unwrap_or_else(|e| panic!("sound as {ext}: {e}"));
            assert!(fs::metadata(&audio).unwrap().len() > 1000, "sound as {ext} is not empty");
            assert!(probe(&audio)["audio_streams"].as_array().is_some_and(|a| !a.is_empty()), "{ext} has an audio stream");
        }

        let no_sound = convert(&spec("wav", &["format=wav"]), &silent, &dir.join("o"));
        assert!(no_sound.as_ref().is_err_and(|e| e.contains("no sound")), "a clip without sound has no audio to take: {no_sound:?}");
        assert!(fs::read_dir(dir.join("o")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with("senza")), "nothing written for the silent clip");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The probes FilmCraft gave for the real files: an old-codec AVI it cannot decode (empty pixel format), and the
    /// videos it can.
    #[test]
    fn a_video_with_no_decoder_is_told_from_one_that_has() {
        let avi: Value = serde_json::from_str(r#"{"audio_streams":[],"container":"AVI","video":{"codec":"CRAM","height":576,"pixel_format":"","width":1024}}"#).unwrap();
        assert_eq!(undecodable_codec(&avi).as_deref(), Some("CRAM"));
        for good in [
            r#"{"audio_streams":[],"video":{"codec":"H.264 (High)","pixel_format":"YUV 4:2:0 8-bit"}}"#,
            r#"{"audio_streams":[],"video":{"codec":"Apple ProRes 422 HQ","pixel_format":"YUV 4:2:2 10-bit"}}"#,
            r#"{"audio_streams":[],"video":{"codec":"GIF","pixel_format":"RGB 8-bit"}}"#,
        ] {
            assert_eq!(undecodable_codec(&serde_json::from_str(good).unwrap()), None, "{good}");
        }
        // a sound file has no video object at all, and a probe that says nothing is not a verdict
        assert_eq!(undecodable_codec(&serde_json::from_str(r#"{"audio_streams":[{"codec":"MP3"}],"video":null}"#).unwrap()), None);
        assert_eq!(undecodable_codec(&serde_json::from_str("{}").unwrap()), None);
        // an uncompressed AVI has no pixel format either, but FilmCraft names its codec in words and reads it
        let raw: Value = serde_json::from_str(r#"{"video":{"codec":"Uncompressed RGB 24-bit","pixel_format":""}}"#).unwrap();
        assert_eq!(undecodable_codec(&raw), None);
        // no codec name at all: the engine is left to try
        let nameless: Value = serde_json::from_str(r#"{"video":{"codec":"","pixel_format":""}}"#).unwrap();
        assert_eq!(undecodable_codec(&nameless), None);
        // another bare four-letter code is a codec it does not know either
        let other: Value = serde_json::from_str(r#"{"video":{"codec":"IV50","pixel_format":""}}"#).unwrap();
        assert_eq!(undecodable_codec(&other).as_deref(), Some("IV50"));
    }

    /// With `VOIDFLARE_TEST_CRAM` (an AVI made with Microsoft Video 1) and FilmCraft: the video is decoded here and
    /// converted, named after the original, and it is the real picture, not FilmCraft's "Media unreadable" card (which
    /// is a flat, mostly red picture: a real clip has far more detail, so the file is far bigger for its length).
    #[test]
    fn real_old_codec_avi_converts_to_mp4() {
        use crate::engine::FILMCRAFT;
        let (Some(path), Some(cli)) = (std::env::var_os("VOIDFLARE_TEST_CRAM"), FILMCRAFT.find()) else { return };
        let spec = Spec { cli, out_ext: "mp4".into(), extra_args: vec!["format=h264".into(), "bitrateMode=crf".into(), "crf=23".into()], env: vec![] };
        let dir = job::scratch_dir().unwrap();
        let made = convert(&spec, Path::new(&path), &dir).expect("converts");
        assert_eq!(made.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert_eq!(made.file_stem(), Path::new(&path).file_stem(), "named after the original, not after the decoded copy");
        let probed = probe(&spec.cli, &made).expect("the result probes");
        assert_eq!(probed["video"]["codec"].as_str().map(|c| c.starts_with("H.264")), Some(true));
        let seconds = probed["duration"].as_f64().unwrap_or(0.0) / 255_255_255_000.0;
        let source = probe(&spec.cli, Path::new(&path)).and_then(|p| p["duration"].as_f64()).unwrap_or(0.0) / 255_255_255_000.0;
        assert!((seconds - source).abs() < 0.3, "{seconds} s against {source} s");
        // the card compresses to about 40 kbit per second; a picture of an ultrasound scan to well over 1 Mbit
        let kbps = fs::metadata(&made).unwrap().len() as f64 * 8.0 / 1000.0 / seconds.max(0.1);
        assert!(kbps > 400.0, "{kbps} kbit/s looks like the empty card");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_line_is_read_from_stdout() {
        let out = "{\"id\":\"file.import\",\"line\":1,\"ok\":true}\n{\"error\":\"nothing to import\",\"id\":\"file.newSequence\",\"line\":2,\"ok\":false}\n";
        assert_eq!(script_error(out).as_deref(), Some("nothing to import"));
        assert_eq!(script_error("{\"ok\":true}"), None);
    }
}
