//! tusk — a small, sharp CLI journal for plain prose.
//!
//! Entries live in one plain-text file and are stored in the widely used
//! `[YYYY-MM-DD HH:MM:SS AM] body` layout (the same format used by `jrnl`),
//! so the file stays greppable, syncable and readable with any other tool.
//!
//! The `tusk` command is the journal; `tuskedit` is the same editor, on its
//! own, for any text file. Both are thin front ends over these modules.

pub mod buffer;
pub mod cli;
pub mod cmd;
pub mod config;
pub mod editor;
pub mod fileedit;
pub mod journal;
pub mod query;
pub mod render;
pub mod spell;
pub mod style;
pub mod text;
pub mod ui;

use std::process::ExitCode;

/// Report a finished command the way command-line tools do: silently on
/// success, and on one clear line to stderr on failure.
pub fn finish(program: &str, outcome: anyhow::Result<()>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            render::note(&format!("{program}: {err:#}"));
            ExitCode::FAILURE
        }
    }
}
