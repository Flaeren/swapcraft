//! Image Converter: PNG, JPG, WebP, TIFF, BMP, GIF and more, offline on the PhotoCraft engine.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::PHOTOCRAFT;
use vf_ui::{Config, FormatDef, QualityDef};

/// Everything the route table (`vf_core::routes`) reads as an image too.
static CONFIG: Config = Config {
    title: "Image Converter",
    engine: PHOTOCRAFT,
    engine_name: "PhotoCraft",
    exts: vf_core::routes::IMAGE_EXTS,
    out_ext: "png",
    formats: &[
        FormatDef { ext: "png", label: "PNG", args: &[] },
        FormatDef { ext: "jpg", label: "JPG", args: &[] },
        FormatDef { ext: "webp", label: "WebP", args: &[] },
        FormatDef { ext: "tif", label: "TIFF (LZW)", args: &["--tiff-compression", "lzw"] },
        FormatDef { ext: "bmp", label: "BMP", args: &[] },
        FormatDef { ext: "gif", label: "GIF", args: &[] },
        FormatDef { ext: "ico", label: "ICO", args: &[] },
        FormatDef { ext: "tga", label: "TGA", args: &[] },
        FormatDef { ext: "psd", label: "PSD (Photoshop)", args: &[] },
    ],
    quality: Some(QualityDef { arg: "--quality", default: 90, exts: &["jpg", "webp"] }),
    runner: None,
    options: &[],
    password_env: None,
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
