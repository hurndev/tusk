use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};

/// A small, sharp CLI journal for plain prose.
#[derive(Parser, Debug)]
#[command(
    name = "tusk",
    version,
    about = "A small, sharp CLI journal for plain prose",
    long_about = "tusk writes plain-prose journal entries to a single text file.\n\
                  Entries are stored as `[YYYY-MM-DD HH:MM:SS AM] body`, the same\n\
                  format used by jrnl, so the file stays portable and greppable.",
    disable_help_subcommand = true,
    after_help = "Run `tusk init` once to choose your journal file.\n\
                  Entry numbers start at 1 and count backwards from the newest entry,\n\
                  so `tusk edit 1` always edits what you wrote most recently."
)]
pub struct Cli {
    /// Journal file to use, overriding the configured one.
    #[arg(long, short = 'f', global = true, value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Disable coloured output.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Do not write a backup copy before modifying the journal.
    #[arg(long, global = true)]
    pub no_backup: bool,

    /// Show more detail (repeat for even more).
    #[arg(long, short = 'v', global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Write an entry: in the editor, from TEXT, or from stdin.
    #[command(visible_aliases = ["write", "compose"], alias = "add")]
    New(NewArgs),

    /// Show entries in full — the newest one by default.
    #[command(visible_alias = "latest")]
    Show(ShowArgs),

    /// List entries, newest first, one line each.
    #[command(visible_alias = "ls")]
    List(ListArgs),

    /// Read the journal chronologically, in the built-in reader.
    Read(ReadArgs),

    /// Edit an entry in your editor (defaults to the newest).
    Edit(EditArgs),

    /// Delete an entry.
    #[command(visible_alias = "rm")]
    Delete(DeleteArgs),

    /// Search entries for words, phrases, regexes, tags or dates.
    #[command(visible_aliases = ["grep", "find"])]
    Search(SearchArgs),

    /// List the tags you use, with counts.
    Tags(TagsArgs),

    /// Summarise the journal: counts, span, streaks, busiest months.
    Stats(StatsArgs),

    /// Open the journal file itself in your editor.
    Open,

    /// Print the journal file path in use.
    Path,

    /// Show or change tusk's configuration.
    #[command(subcommand)]
    Config(ConfigArgs),

    /// Set up tusk: choose the journal file and editor.
    Init(InitArgs),
}

/// Which editor to compose in. Flattened into the commands that write.
#[derive(Args, Debug, Default, Clone)]
pub struct EditorOpts {
    /// Compose in tusk's built-in editor (the default).
    #[arg(long, conflicts_with_all = ["external", "editor_cmd"])]
    pub builtin: bool,

    /// Compose in an external editor instead of the built-in one.
    #[arg(long, conflicts_with = "editor_cmd")]
    pub external: bool,

    /// Use this editor command instead of the built-in editor.
    #[arg(long = "editor", value_name = "CMD")]
    pub editor_cmd: Option<String>,
}

#[derive(Args, Debug, Default, Clone)]
pub struct FilterArgs {
    /// Only entries on or after this date: 2026, 2026-03, 2026-03-14, today, -7d.
    #[arg(long, value_name = "DATE", alias = "since")]
    pub from: Option<String>,

    /// Only entries on or before this date.
    #[arg(long, value_name = "DATE", alias = "until")]
    pub to: Option<String>,

    /// Only entries tagged #tag or @tag.
    #[arg(long, short = 't', value_name = "TAG")]
    pub tag: Option<String>,

    /// Oldest first instead of newest first.
    #[arg(long)]
    pub reverse: bool,
}

#[derive(Args, Debug)]
pub struct NewArgs {
    /// Write this text instead of opening an editor (or reading stdin).
    #[arg(value_name = "TEXT", num_args = 0..)]
    pub text: Vec<String>,

    /// Timestamp to record instead of now, e.g. "2026-03-14 21:30".
    #[arg(long, short = 'd', value_name = "WHEN")]
    pub date: Option<String>,

    #[command(flatten)]
    pub editor: EditorOpts,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Show at most N entries (0 = all).
    #[arg(long, short = 'n', value_name = "N", default_value_t = 0)]
    pub limit: usize,

    /// Group entries under month headings.
    #[arg(long)]
    pub group: bool,

    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Entries to show: numbers (1 = newest), dates, or latest/oldest.
    #[arg(value_name = "ENTRY", num_args = 0.., conflicts_with = "last")]
    pub ids: Vec<String>,

    /// Show the newest N entries (0 = all).
    #[arg(long = "last", short = 'n', value_name = "N", default_value_t = 1)]
    pub last: usize,

    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug)]
pub struct ReadArgs {
    /// Entries to read: numbers, dates, or keywords. Defaults to everything.
    #[arg(value_name = "ENTRY", num_args = 0..)]
    pub ids: Vec<String>,

    /// Print the journal file byte for byte, without headings or wrapping.
    #[arg(long)]
    pub raw: bool,

    /// Print wrapped text on stdout instead of opening the reader.
    #[arg(long, alias = "no-pager")]
    pub print: bool,

    /// Read through your external pager instead of the built-in reader.
    #[arg(long)]
    pub pager: bool,

    /// Prose width in columns for this run.
    #[arg(long, value_name = "N")]
    pub width: Option<usize>,

    #[command(flatten)]
    pub filter: FilterArgs,

    #[command(flatten)]
    pub editor: EditorOpts,
}

#[derive(Args, Debug)]
pub struct EditArgs {
    /// Entry number (1 = newest), date prefix, or latest/oldest. Default: newest.
    #[arg(value_name = "ENTRY")]
    pub id: Option<String>,

    /// Change the entry's timestamp, e.g. "2026-03-14 21:30".
    #[arg(long, short = 'd', value_name = "WHEN")]
    pub date: Option<String>,

    #[command(flatten)]
    pub editor: EditorOpts,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    /// Entry number (1 = newest), date prefix, or latest/oldest.
    #[arg(value_name = "ENTRY")]
    pub id: Option<String>,

    /// Skip the confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Show what would be deleted without deleting it.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Words to look for. All must appear unless --any is given.
    #[arg(value_name = "QUERY", num_args = 0..)]
    pub query: Vec<String>,

    /// Treat the query as a regular expression.
    #[arg(long, short = 'r')]
    pub regex: bool,

    /// Match case-sensitively.
    #[arg(long, short = 's')]
    pub case_sensitive: bool,

    /// Match any query word instead of all of them.
    #[arg(long, short = 'a')]
    pub any: bool,

    /// Print the matching entries in full.
    #[arg(long)]
    pub full: bool,

    /// Print only the number of matching entries.
    #[arg(long, short = 'c')]
    pub count: bool,

    /// Show at most N matches (0 = all).
    #[arg(long, short = 'n', value_name = "N", default_value_t = 0)]
    pub limit: usize,

    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug)]
pub struct TagsArgs {
    /// Show at most N tags (0 = all).
    #[arg(long, short = 'n', value_name = "N", default_value_t = 0)]
    pub limit: usize,

    /// Sort alphabetically instead of by frequency.
    #[arg(long)]
    pub alpha: bool,

    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug)]
pub struct StatsArgs {
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Subcommand, Debug)]
pub enum ConfigArgs {
    /// Print the configuration file and its effective values.
    Show,

    /// Print one configuration value.
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// Set a value: journal, editor, pager, backup.
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },

    /// Remove a value, returning it to its default.
    Unset {
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// Edit the configuration file in your editor.
    Edit,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Journal file to use.
    #[arg(long, short = 'f', value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Editor command to use for composing entries.
    #[arg(long, short = 'e', value_name = "CMD")]
    pub editor: Option<String>,

    /// Accept the detected defaults without prompting.
    #[arg(long, short = 'y')]
    pub yes: bool,
}
