//! Shared core of the voidflare apps: languages, engine discovery, file collection and the job that
//! runs an ArtCraft command-line engine on one file. Nothing here draws anything.

pub mod dwg;
pub mod ebook;
pub mod embedded;
pub mod engine;
pub mod epub;
pub mod film;
pub mod i18n;
pub mod job;
pub mod markup;
pub mod menu;
pub mod mobi;
pub mod packs;
pub mod pdf;
pub mod pdfmerge;
pub mod platform;
pub mod recipe;
pub mod routes;
pub mod shortcuts;
pub mod update;

pub use i18n::{t, tf};
