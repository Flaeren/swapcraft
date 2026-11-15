//! Mail Converter: e-mail messages (EML), mailboxes (MBOX) and Outlook data files (PST) to PDF, to single EML
//! messages or to a mailbox, and the calendar and contacts of a PST. The messages are read in Rust
//! (`vf_core::mail`); WordCraft lays the PDFs out; MailCraft opens the PST files. Pictures stored on the internet are
//! never downloaded.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vf_core::engine::WORDCRAFT;
use vf_ui::{Config, FormatDef};

static CONFIG: Config = Config {
    title: "Mail Converter",
    engine: WORDCRAFT,
    engine_name: "WordCraft",
    exts: vf_core::mail::SOURCE_EXTS,
    out_ext: "pdf",
    formats: &[
        FormatDef { ext: "pdf", label: "PDF", args: &[] },
        FormatDef { ext: "eml", label: "EML messages", args: &[] },
        FormatDef { ext: "mbox", label: "MBOX", args: &[] },
        FormatDef { ext: "ics", label: "Calendar (ICS), from PST", args: &[] },
        FormatDef { ext: "vcf", label: "Contacts (VCF), from PST", args: &[] },
    ],
    quality: None,
    runner: Some(vf_core::mail::convert),
    options: &[],
    password_env: Some(vf_core::mail::PST_PASSWORD_ENV),
    ask_password: Some(vf_core::mail::password_error),
    icon_256: include_bytes!("../assets/icon_256.rgba"),
    icon_128: include_bytes!("../assets/icon_128.rgba"),
    catalog: include_str!("../assets/strings.tsv"),
};

fn main() -> eframe::Result {
    vf_ui::run(&CONFIG)
}
