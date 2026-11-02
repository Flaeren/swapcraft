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

/// Whether the file has an audio stream, by FilmCraft's own `probe`; `None` when the probe says nothing usable.
fn has_audio(cli: &Path, src: &Path) -> Option<bool> {
    let tmp = job::scratch_dir().ok()?;
    let mut cmd = Command::new(cli);
    cmd.arg("probe").arg(src);
    let out = job::run_capture(&mut cmd, &tmp).ok();
    let _ = fs::remove_dir_all(&tmp);
    let v: Value = serde_json::from_str(&out?).ok()?;
    Some(!v["audio_streams"].as_array()?.is_empty())
}

/// Output types that carry only sound.
const AUDIO_ONLY: &[&str] = &["wav", "m4a", "aac", "flac", "aiff"];

/// Converts one video (or takes the sound of it) into `spec.out_ext`, into `out_dir`.
pub fn convert(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let params = export_params(&spec.extra_args)?;
    // FilmCraft would write a silent file for a clip with no sound; say so instead.
    if AUDIO_ONLY.contains(&spec.out_ext.as_str()) && has_audio(&spec.cli, src) == Some(false) {
        // Translated in the apps; the command line and the MCP server load no catalog, so they get the English.
        let said = crate::t("film.no_sound");
        return Err(if said == "film.no_sound" { "This video has no sound to take.".to_owned() } else { said });
    }
    let tmp = job::scratch_dir().map_err(|e| e.to_string())?;
    let result = run_in(&tmp, spec, src, out_dir, params);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn run_in(tmp: &Path, spec: &Spec, src: &Path, out_dir: &Path, params: Map<String, Value>) -> Result<PathBuf, String> {
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
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
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

        for (ext, format) in [("wav", "wav"), ("m4a", "aac"), ("flac", "flac")] {
            let audio = convert(&spec(ext, &[&format!("format={format}")]), &src, &dir.join("o")).unwrap_or_else(|e| panic!("sound as {ext}: {e}"));
            assert!(fs::metadata(&audio).unwrap().len() > 1000, "sound as {ext} is not empty");
            assert!(probe(&audio)["audio_streams"].as_array().is_some_and(|a| !a.is_empty()), "{ext} has an audio stream");
        }

        let no_sound = convert(&spec("wav", &["format=wav"]), &silent, &dir.join("o"));
        assert!(no_sound.as_ref().is_err_and(|e| e.contains("no sound")), "a clip without sound has no audio to take: {no_sound:?}");
        assert!(fs::read_dir(dir.join("o")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with("senza")), "nothing written for the silent clip");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_line_is_read_from_stdout() {
        let out = "{\"id\":\"file.import\",\"line\":1,\"ok\":true}\n{\"error\":\"nothing to import\",\"id\":\"file.newSequence\",\"line\":2,\"ok\":false}\n";
        assert_eq!(script_error(out).as_deref(), Some("nothing to import"));
        assert_eq!(script_error("{\"ok\":true}"), None);
    }
}
