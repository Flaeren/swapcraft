//! Excel to PDF: spreadsheets and CSV to PDF, Excel or CSV offline on the GridCraft engine. `vf_core::sheet` writes
//! one CSV per sheet and reads a CSV from a European Excel (`;`, decimal comma, day-first dates) as meant.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::GRIDCRAFT;
use vf_ui::{Config, FormatDef};

static CONFIG: Config = Config {
    title: "Excel to PDF",
    engine: GRIDCRAFT,
    engine_name: "GridCraft",
    exts: &["xlsx", "csv", "tsv"],
    out_ext: "pdf",
    formats: &[
        FormatDef { ext: "pdf", label: "PDF", args: &[] },
        FormatDef { ext: "xlsx", label: "Excel (XLSX)", args: &[] },
        FormatDef { ext: "csv", label: "CSV · one file per sheet", args: &[] },
    ],
    quality: None,
    runner: Some(vf_core::sheet::convert),
    options: &[],
    password_env: None,
    ask_password: None,
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
