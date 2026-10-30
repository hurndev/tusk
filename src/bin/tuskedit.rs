//! The `tuskedit` command: tusk's editor, on its own, for any text file.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use tusk::config;
use tusk::fileedit;
use tusk::style::Paint;

/// Edit a text file in tusk's editor.
#[derive(Parser, Debug)]
#[command(
    name = "tuskedit",
    version,
    about = "Edit a text file in tusk's built-in editor",
    long_about = "tuskedit opens any UTF-8 text file in the editor tusk uses for journal\n\
                  entries: soft wrapping, undo, word-kill, and F2 for spellchecking.\n\
                  Name a file that does not exist yet and it is created on the first save.",
    after_help = "Files are written to a temporary file and renamed into place, so an\n\
                  interrupted save cannot leave a half-written file.\n\
                  \n\
                  To write a journal entry instead, use `tusk new`."
)]
struct Cli {
    /// The file to edit. Created on the first save if it does not exist.
    #[arg(value_name = "PATH")]
    path: PathBuf,

    /// Disable coloured output.
    #[arg(long)]
    no_color: bool,

    /// Do not write a backup copy when the file happens to be tusk's journal.
    #[arg(long)]
    no_backup: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = config::load();
    let paint = Paint::new(cli.no_color);
    // A journal may not be configured at all; that is fine here.
    let journal = config::resolve(None, &cfg).ok().map(|(path, _)| path);
    let options = fileedit::Options {
        program: "tuskedit",
        no_backup: cli.no_backup,
        journal: journal.as_deref(),
        editor: fileedit::Pick::Builtin,
    };
    tusk::finish(
        "tuskedit",
        fileedit::edit(&cli.path, &cfg, &paint, &options),
    )
}
