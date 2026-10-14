//! tusk — a small, sharp CLI journal for plain prose.
//!
//! Entries live in one plain-text file and are stored in the widely used
//! `[YYYY-MM-DD HH:MM:SS AM] body` layout (the same format used by `jrnl`),
//! so the file stays greppable, syncable and readable with any other tool.

mod buffer;
mod cli;
mod cmd;
mod config;
mod editor;
mod journal;
mod query;
mod render;
mod style;
mod text;
mod ui;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(err) = cmd::run(cli) {
        render::note(&format!("tusk: {err:#}"));
        std::process::exit(1);
    }
}
