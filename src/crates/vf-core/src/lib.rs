//! Shared core of the voidflare apps: languages, engine discovery, file collection and the job that
//! runs an ArtCraft command-line engine on one file. Nothing here draws anything.

pub mod dwg;
pub mod engine;
pub mod i18n;
pub mod job;
pub mod pdf;
pub mod pdfmerge;
pub mod update;

pub use i18n::{t, tf};
