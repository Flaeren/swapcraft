//! Audio Converter: most audio files to WAV, FLAC, AIFF, MP3 or M4A, with resampling and normalising, offline.
//! SoundCraft reads many formats (MP3, OGG, M4A, AAC...) but writes only WAV, FLAC and AIFF; MP3 and M4A come from
//! the FilmCraft of Video Converter (`vf_core::audio`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::SOUNDCRAFT;
use vf_ui::{Config, FormatDef, OptionDef};

static CONFIG: Config = Config {
    title: "Audio Converter",
    engine: SOUNDCRAFT,
    engine_name: "SoundCraft",
    exts: vf_core::routes::AUDIO_EXTS,
    out_ext: "wav",
    formats: &[
        FormatDef { ext: "wav", label: "WAV · 16-bit · 44.1 kHz (CD)", args: &["--bit-depth", "16", "--rate", "44100"] },
        FormatDef { ext: "wav", label: "WAV · 24-bit (keep the sample rate)", args: &["--bit-depth", "24"] },
        FormatDef { ext: "wav", label: "WAV · 32-bit float", args: &["--bit-depth", "32f"] },
        FormatDef { ext: "flac", label: "FLAC · 16-bit", args: &["--bit-depth", "16"] },
        FormatDef { ext: "flac", label: "FLAC · 24-bit", args: &["--bit-depth", "24"] },
        FormatDef { ext: "aiff", label: "AIFF · 16-bit", args: &["--bit-depth", "16"] },
        FormatDef { ext: "aiff", label: "AIFF · 24-bit", args: &["--bit-depth", "24"] },
        FormatDef { ext: "mp3", label: "MP3 · 320 kbps", args: &[] },
        FormatDef { ext: "m4a", label: "M4A · AAC", args: &[] },
    ],
    quality: None,
    runner: Some(vf_core::audio::convert),
    options: &[
        OptionDef { label_key: "app.opt_normalize", arg: "--normalize", default: false },
        OptionDef { label_key: "app.opt_loudness", arg: vf_core::audio::LOUDNESS, default: false },
    ],
    password_env: None,
    ask_password: None,
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
