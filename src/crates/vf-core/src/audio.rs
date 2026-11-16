//! Audio Converter's conversions. SoundCraft writes WAV, FLAC and AIFF; MP3 and M4A (AAC) come from FilmCraft,
//! which ships with Video Converter and is found beside it (see [`crate::engine::Engine::find`]).
//!
//! FilmCraft is kept to the lossy formats on purpose: it writes 16-bit WAV and FLAC whatever it reads, so a
//! 24-bit file would lose resolution through it. For the same reason the even-loudness setting, which only
//! FilmCraft has, applies to MP3 and M4A alone.

use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::FILMCRAFT;
use crate::job::{self, Spec};

/// The app's own setting for even loudness: never passed to an engine, read here.
pub const LOUDNESS: &str = "--loudness";

/// The loudness MP3 and M4A are brought to with [`LOUDNESS`], in LUFS: the level Apple Podcasts asks for.
pub const TARGET_LUFS: &str = "-16.0";

/// The FilmCraft export format for an output type it writes for this app.
fn film_format(out_ext: &str) -> Option<&'static str> {
    match out_ext {
        "mp3" => Some("mp3"),
        "m4a" => Some("aac"),
        _ => None,
    }
}

/// How one file is made: by SoundCraft alone, or by FilmCraft after an optional SoundCraft pass (the peak
/// normalising and the resampling only SoundCraft does).
#[derive(Debug, PartialEq)]
enum Plan {
    Sound(Vec<String>),
    Film { before: Option<Vec<String>>, export: Vec<String> },
}

fn plan(out_ext: &str, args: &[String]) -> Plan {
    let Some(format) = film_format(out_ext) else {
        return Plan::Sound(args.iter().filter(|a| *a != LOUDNESS).cloned().collect());
    };
    let loud = args.iter().any(|a| a == LOUDNESS);
    let mut before = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            // Even loudness sets the level itself; a peak normalised first would change nothing it keeps.
            "--normalize" if !loud => before.push(a.clone()),
            "--rate" => {
                before.push(a.clone());
                before.extend(it.next().cloned());
            }
            _ => {}
        }
    }
    let mut export = vec![format!("format={format}")];
    if loud {
        export.push(format!("loudnessLufs={TARGET_LUFS}"));
    }
    let before = (!before.is_empty()).then(|| [vec!["--bit-depth".to_owned(), "24".to_owned()], before].concat());
    Plan::Film { before, export }
}

/// Converts one file into `spec.out_ext` (`spec.cli` is SoundCraft), into `out_dir`.
pub fn convert(spec: &Spec, src: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    match plan(&spec.out_ext, &spec.extra_args) {
        Plan::Sound(args) => job::convert_one(&Spec { extra_args: args, ..spec.clone() }, src, out_dir),
        Plan::Film { before, export } => {
            let film = FILMCRAFT.find().ok_or_else(need_filmcraft)?;
            let film_spec = Spec { cli: film, out_ext: spec.out_ext.clone(), extra_args: export, env: vec![] };
            let Some(args) = before else {
                return crate::film::convert(&film_spec, src, out_dir);
            };
            let tmp = job::scratch_dir().map_err(|e| e.to_string())?;
            let result = job::convert_one(&Spec { out_ext: "wav".into(), extra_args: args, ..spec.clone() }, src, &tmp)
                .and_then(|wav| crate::film::convert(&film_spec, &wav, out_dir));
            let _ = fs::remove_dir_all(&tmp);
            result
        }
    }
}

/// MP3 or M4A asked for without FilmCraft on this PC.
fn need_filmcraft() -> String {
    // Translated in the app; the command line and the MCP server load no catalog, so they get the English.
    let said = crate::t("audio.need_filmcraft");
    if said == "audio.need_filmcraft" {
        "MP3 and M4A are made by the FilmCraft engine, which comes with Video Converter. Install Video Converter from SwapCraft, then try again."
            .to_owned()
    } else {
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn lossless_formats_stay_with_soundcraft_and_lose_the_loudness_flag() {
        assert_eq!(plan("flac", &s(&["--normalize", "--loudness", "--bit-depth", "24"])), Plan::Sound(s(&["--normalize", "--bit-depth", "24"])));
        assert_eq!(plan("wav", &s(&["--bit-depth", "16", "--rate", "44100"])), Plan::Sound(s(&["--bit-depth", "16", "--rate", "44100"])));
    }

    #[test]
    fn mp3_and_m4a_go_to_filmcraft() {
        assert_eq!(plan("mp3", &[]), Plan::Film { before: None, export: s(&["format=mp3"]) });
        assert_eq!(plan("m4a", &s(&["--loudness"])), Plan::Film { before: None, export: s(&["format=aac", "loudnessLufs=-16.0"]) });
    }

    /// A quiet 1 s stereo tone (about -36 dBFS RMS), 16-bit 44.1 kHz.
    fn quiet_wav() -> Vec<u8> {
        let rate = 44_100u32;
        let mut data = Vec::new();
        for n in 0..rate {
            let v = ((n as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 700.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&[1, 0, 2, 0]);
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 4).to_le_bytes());
        b.extend_from_slice(&[4, 0, 16, 0]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend(data);
        b
    }

    /// RMS level in dBFS of a 16-bit PCM WAV (the samples after the 44-byte header SoundCraft writes).
    fn rms_db(wav: &Path) -> f64 {
        let b = fs::read(wav).unwrap();
        let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
        let s: Vec<f64> = b[at..].as_chunks::<2>().0.iter().map(|c| f64::from(i16::from_le_bytes(*c))).collect();
        let rms = (s.iter().map(|x| x * x).sum::<f64>() / s.len() as f64).sqrt();
        20.0 * (rms / 32768.0).log10()
    }

    /// MP3 and M4A with the real engines: plain, after SoundCraft's peak pass, and brought to -16 LUFS (skipped
    /// without SoundCraft or FilmCraft).
    #[test]
    fn mp3_and_m4a_with_the_real_engines() {
        let (Some(sound), Some(_)) = (crate::engine::SOUNDCRAFT.find(), FILMCRAFT.find()) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("vf_audio_lossy_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("voce città.wav");
        fs::write(&src, quiet_wav()).unwrap();
        let spec = |ext: &str, args: &[&str]| Spec { cli: sound.clone(), out_ext: ext.into(), extra_args: s(args), env: vec![] };
        let level = |file: &Path, name: &str| {
            let wav = dir.join(format!("{name}.wav"));
            let mut cmd = std::process::Command::new(&sound);
            cmd.args(["convert", "--bit-depth", "16"]).arg(file).arg(&wav);
            assert!(cmd.output().unwrap().status.success(), "{name} decodes");
            rms_db(&wav)
        };
        let before = rms_db(&src);

        let plain = convert(&spec("mp3", &[]), &src, &dir.join("plain")).expect("mp3");
        assert_eq!(plain.file_name().unwrap(), "voce città.mp3");
        let head = fs::read(&plain).unwrap();
        assert!(head.starts_with(b"ID3") || (head[0] == 0xFF && head[1] & 0xE0 == 0xE0), "an MPEG audio file");
        assert!((level(&plain, "plain") - before).abs() < 1.0, "plain MP3 keeps the level");

        let peak = convert(&spec("mp3", &["--normalize"]), &src, &dir.join("peak")).expect("mp3 after the peak pass");
        assert!(level(&peak, "peak") > before + 20.0, "peak to -0.1 dBFS raises a -36 dB tone");

        let loud = convert(&spec("m4a", &[LOUDNESS]), &src, &dir.join("loud")).expect("m4a at -16 LUFS");
        let got = level(&loud, "loud");
        assert!(got > before + 10.0 && got < -10.0, "brought near -16 LUFS: {before:.1} -> {got:.1} dBFS RMS");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn soundcraft_settings_run_first_and_loudness_wins_over_the_peak() {
        assert_eq!(
            plan("mp3", &s(&["--normalize"])),
            Plan::Film { before: Some(s(&["--bit-depth", "24", "--normalize"])), export: s(&["format=mp3"]) }
        );
        assert_eq!(plan("mp3", &s(&["--normalize", "--loudness"])), Plan::Film { before: None, export: s(&["format=mp3", "loudnessLufs=-16.0"]) });
        assert_eq!(
            plan("m4a", &s(&["--rate", "48000", "--loudness"])),
            Plan::Film { before: Some(s(&["--bit-depth", "24", "--rate", "48000"])), export: s(&["format=aac", "loudnessLufs=-16.0"]) }
        );
    }
}
