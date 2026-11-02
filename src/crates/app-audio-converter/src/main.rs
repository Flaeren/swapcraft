//! Audio Converter: most audio files to WAV, FLAC or AIFF, with resampling and normalising, offline on the
//! SoundCraft engine. SoundCraft reads many formats (MP3, OGG, M4A, AAC...) but writes only WAV, FLAC and AIFF.
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
    ],
    quality: None,
    runner: None,
    options: &[OptionDef { label_key: "app.opt_normalize", arg: "--normalize", default: false }],
    password_env: None,
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
