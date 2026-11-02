//! Shared look and shared windows of the voidflare apps.

pub mod shell;
pub mod theme;
pub mod widgets;

pub use shell::{Config, FormatDef, OptionDef, QualityDef, Runner, run};
