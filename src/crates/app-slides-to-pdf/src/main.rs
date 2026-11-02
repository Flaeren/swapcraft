//! Slides to PDF: PowerPoint presentations to PDF offline on the DeckCraft engine.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::DECKCRAFT;
use vf_ui::Config;

static CONFIG: Config = Config {
    title: "Slides to PDF",
    engine: DECKCRAFT,
    engine_name: "DeckCraft",
    exts: &["pptx", "deckcraft"],
    out_ext: "pdf",
    formats: &[],
    quality: None,
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
