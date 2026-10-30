//! Word to PDF: DOCX, DOC, RTF and ODT to PDF on the WordCraft engine, offline.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::WORDCRAFT;
use vf_ui::{Config, OptionDef};

static CONFIG: Config = Config {
    title: "Word to PDF",
    engine: WORDCRAFT,
    engine_name: "WordCraft",
    exts: &["docx", "doc", "rtf", "odt"],
    out_ext: "pdf",
    options: &[
        // Off by default: a PDF should show the document as it was saved.
        OptionDef { label_key: "app.opt_fields", arg: "--update-fields", default: false },
        OptionDef { label_key: "app.opt_markup", arg: "--markup", default: false },
    ],
    password_env: Some("WORDCRAFT_PASSWORD"),
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
