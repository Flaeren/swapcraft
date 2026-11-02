//! E-book Converter: EPUB and Kindle books (AZW3, AZW, MOBI, PRC) without DRM, text, HTML, Markdown and word
//! processor files, to EPUB, text, HTML, PDF or Word. The reading and the EPUB writing are Rust (`vf_core::epub`,
//! `vf_core::mobi`); PDF and Word are made by WordCraft, which is also what reads Word files and Markdown.
//! A book with DRM is refused with a clear message: nothing here removes copy protection.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::WORDCRAFT;
use vf_ui::{Config, FormatDef};

static CONFIG: Config = Config {
    title: "E-book Converter",
    engine: WORDCRAFT,
    engine_name: "WordCraft",
    exts: vf_core::ebook::SOURCE_EXTS,
    out_ext: "epub",
    formats: &[
        FormatDef { ext: "epub", label: "EPUB", args: &[] },
        FormatDef { ext: "txt", label: "Text (TXT)", args: &[] },
        FormatDef { ext: "html", label: "HTML (one page, pictures inside)", args: &[] },
        FormatDef { ext: "pdf", label: "PDF", args: &[] },
        FormatDef { ext: "docx", label: "Word (DOCX)", args: &[] },
    ],
    quality: None,
    runner: Some(vf_core::ebook::convert),
    options: &[],
    password_env: None,
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
