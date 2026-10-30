//! The `tusk` command: everything the journal does.

use std::process::ExitCode;

use clap::Parser;

use tusk::cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    tusk::finish("tusk", tusk::cmd::run(cli))
}
