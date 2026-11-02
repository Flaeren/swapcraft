//! Video Converter: most video files to MP4 (H.264), ProRes MOV or GIF, or just their sound, offline on the
//! FilmCraft engine. FilmCraft is an editor: each conversion is an import, a sequence and an export (`vf_core::film`).
//! H.265 is left out on purpose: FilmCraft writes it only through a hardware encoder (NVIDIA), which most PCs lack.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::FILMCRAFT;
use vf_ui::{Config, FormatDef, OptionDef};

static CONFIG: Config = Config {
    title: "Video Converter",
    engine: FILMCRAFT,
    engine_name: "FilmCraft",
    exts: vf_core::routes::VIDEO_EXTS,
    out_ext: "mp4",
    formats: &[
        FormatDef { ext: "mp4", label: "MP4 · H.264 · good quality", args: &["format=h264", "bitrateMode=crf", "crf=20"] },
        FormatDef { ext: "mp4", label: "MP4 · H.264 · small file", args: &["format=h264", "bitrateMode=crf", "crf=28"] },
        FormatDef { ext: "mov", label: "MOV · ProRes 422 HQ (for editing)", args: &["format=prores"] },
        FormatDef { ext: "gif", label: "GIF · animated, no sound", args: &["format=gif", "audio=false"] },
        FormatDef { ext: "wav", label: "Sound only · WAV", args: &["format=wav"] },
        FormatDef { ext: "m4a", label: "Sound only · AAC (M4A)", args: &["format=aac"] },
        FormatDef { ext: "flac", label: "Sound only · FLAC", args: &["format=flac"] },
    ],
    quality: None,
    runner: Some(vf_core::film::convert),
    options: &[
        OptionDef { label_key: "app.opt_half", arg: "scale=0.5", default: false },
        OptionDef { label_key: "app.opt_mute", arg: "audio=false", default: false },
    ],
    password_env: None,
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
