//! Command handlers.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Local, NaiveDate, NaiveDateTime};

use crate::buffer::Buffer;
use crate::cli::*;
use crate::config::{self, Config};
use crate::editor::{self, EditorChoice};
use crate::journal::{self, Entry, Journal};
use crate::query::{Filters, Query};
use crate::render::{self, note, say};
use crate::style::Paint;
use crate::ui::{self, reader::Doc};

pub fn run(mut cli: Cli) -> Result<()> {
    let cfg = config::load();
    let paint = Paint::new(cli.no_color);
    let command = cli.command.take();

    match command {
        None => {
            let args = ShowArgs {
                ids: Vec::new(),
                last: 1,
                filter: FilterArgs::default(),
            };
            cmd_show(&cli, &cfg, &paint, &args)
        }
        Some(Command::New(args)) => cmd_new(&cli, &cfg, &paint, &args),
        Some(Command::Show(args)) => cmd_show(&cli, &cfg, &paint, &args),
        Some(Command::List(args)) => cmd_list(&cli, &cfg, &paint, &args),
        Some(Command::Read(args)) => cmd_read(&cli, &cfg, &args),
        Some(Command::Edit(args)) => cmd_edit(&cli, &cfg, &paint, &args),
        Some(Command::Delete(args)) => cmd_delete(&cli, &cfg, &paint, &args),
        Some(Command::Search(args)) => cmd_search(&cli, &cfg, &paint, &args),
        Some(Command::Tags(args)) => cmd_tags(&cli, &cfg, &paint, &args),
        Some(Command::Stats(args)) => cmd_stats(&cli, &cfg, &paint, &args),
        Some(Command::Open(args)) => cmd_open(&cli, &cfg, &paint, &args),
        Some(Command::Path) => cmd_path(&cli, &cfg),
        Some(Command::Config(args)) => cmd_config(&cli, &cfg, &paint, &args),
        Some(Command::Init(args)) => cmd_init(&cfg, &paint, &args),
    }
}

// ---------------------------------------------------------------- helpers

/// Pick the editor, and refuse politely when the built-in one has no terminal
/// to draw in. A configured external editor is used instead; an explicit
/// request for the built-in editor is never quietly swapped out.
fn choose_editor(flags: &EditorOpts, cfg: &Config, hint: &str) -> Result<EditorChoice> {
    let choice = editor::choose(flags, cfg);
    if ui::frame::interactive() {
        return Ok(choice);
    }
    match choice {
        EditorChoice::Builtin => match editor::explicit_editor_command(cfg) {
            Some(command) => Ok(EditorChoice::External(command)),
            None => bail!("the built-in editor needs a terminal\n  {hint}"),
        },
        EditorChoice::BuiltinRequested => bail!(
            "the built-in editor needs a terminal, and tusk was told to use it\n  \
             (config `editor`, $TUSK_EDITOR, --builtin or --editor builtin)\n  \
             name an external editor instead, or {hint}"
        ),
        other => Ok(other),
    }
}

fn open_journal(cli: &Cli, cfg: &Config) -> Result<Journal> {
    let path = config::resolve_path(cli.file.as_deref(), cfg)?;
    Journal::load(&path)
}

fn filters_from(args: &FilterArgs) -> Result<Filters> {
    let from = match &args.from {
        Some(spec) => Some(crate::query::parse_range(spec)?.0),
        None => None,
    };
    let to = match &args.to {
        Some(spec) => Some(crate::query::parse_range(spec)?.1),
        None => None,
    };
    Ok(Filters {
        from,
        to,
        tag: args.tag.clone(),
        terms: Vec::new(),
        regex: false,
        case_sensitive: false,
        any: false,
    })
}

fn full_query(
    filter: &FilterArgs,
    terms: Vec<String>,
    regex: bool,
    case_sensitive: bool,
    any: bool,
) -> Result<Query> {
    let mut filters = filters_from(filter)?;
    filters.terms = terms;
    filters.regex = regex;
    filters.case_sensitive = case_sensitive;
    filters.any = any;
    Query::new(filters)
}

/// Matching entry indices in file order (oldest first).
fn matching(journal: &Journal, query: &Query) -> Vec<usize> {
    journal
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.matches(entry))
        .map(|(i, _)| i)
        .collect()
}

/// Newest-first display order, honouring `--reverse`.
fn display_order(mut indices: Vec<usize>, reverse: bool) -> Vec<usize> {
    if !reverse {
        indices.reverse();
    }
    indices
}

/// Apply a `0 = all` limit.
fn limited(indices: Vec<usize>, limit: usize) -> Vec<usize> {
    if limit == 0 {
        indices
    } else {
        indices.into_iter().take(limit).collect()
    }
}

fn describe_limit(shown: usize, total: usize) -> String {
    if shown < total {
        format!("{shown} of {total} entries")
    } else if total == 1 {
        "1 entry".to_string()
    } else {
        format!("{total} entries")
    }
}

fn check_header_collision(body: &str, paint: &Paint) {
    if journal::looks_like_header(body) {
        note(&paint.warn(
            "tusk: warning: a line of this entry looks like an entry header — \
             it will be read as a separate entry next time",
        ));
    }
}

fn report_write(journal: &Journal, entry: &Entry, paint: &Paint, verb: &str) {
    let tags = entry.tags();
    let detail = if tags.is_empty() {
        String::new()
    } else {
        let unique: Vec<String> = {
            let mut seen = HashSet::new();
            tags.into_iter()
                .filter(|t| seen.insert(t.to_lowercase()))
                .collect()
        };
        format!("  {}", paint.dim(&unique.join(" ")))
    };
    say(&format!(
        "{} {} ({} words, {}){}{}",
        paint.ok("✓"),
        paint.bold(verb),
        entry.words(),
        entry.date.format(journal::STAMP_SHORT),
        detail,
        paint.dim(&format!("  →  {}", journal.path.display()))
    ));
}

// ---------------------------------------------------------------- commands

fn cmd_new(cli: &Cli, cfg: &Config, paint: &Paint, args: &NewArgs) -> Result<()> {
    let date = match &args.date {
        Some(spec) => journal::parse_datetime_spec(spec)?,
        None => Local::now().naive_local(),
    };
    if !args.text.is_empty() {
        return append_entry(cli, cfg, paint, date, &args.text.join(" "));
    }

    // Piped input means the text is on its way, not that we should open an
    // editor — `cat notes | tusk new` should just work.
    if !std::io::stdin().is_terminal() {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("cannot read stdin")?;
        let body = editor::clean_text(&buffer);
        if !body.trim().is_empty() {
            return append_entry(cli, cfg, paint, date, &body);
        }
        // Nothing arrived on stdin; fall through so the editor path can
        // explain what it needs.
    }

    let mut journal = open_journal(cli, cfg)?;
    let title = format!("new entry  ·  {}", date.format(journal::STAMP_SHORT));
    let draft = Draft {
        date,
        editing: None,
        title: &title,
        editor: &args.editor,
    };
    let outcome = compose(cli, cfg, paint, &mut journal, &draft)?;
    match outcome {
        Compose::Saved(index) => {
            let entry = journal.entries[index].clone();
            report_write(&journal, &entry, paint, "added");
            Ok(())
        }
        Compose::Unchanged => {
            say(&paint.dim("tusk: nothing written"));
            Ok(())
        }
    }
}

fn append_entry(
    cli: &Cli,
    cfg: &Config,
    paint: &Paint,
    date: NaiveDateTime,
    body: &str,
) -> Result<()> {
    check_header_collision(body, paint);
    let mut journal = open_journal(cli, cfg)?;
    journal.append(date, body);
    journal.save(!cli.no_backup)?;
    let entry = journal.entries.last().expect("just appended").clone();
    report_write(&journal, &entry, paint, "added");
    Ok(())
}

fn cmd_show(cli: &Cli, cfg: &Config, paint: &Paint, args: &ShowArgs) -> Result<()> {
    let journal = open_journal(cli, cfg)?;
    if journal.is_empty() {
        say(&paint.dim("tusk: the journal is empty — write something with `tusk new`"));
        return Ok(());
    }
    let total = journal.entries.len();
    let width = output_width(cfg, None);
    let wrap = wrap_output();

    let indices: Vec<usize> = if args.ids.is_empty() {
        // No entry named: the newest N, honouring any filters.
        let query = full_query(&args.filter, Vec::new(), false, false, false)?;
        let matches = matching(&journal, &query);
        if matches.is_empty() {
            say(&paint.dim("tusk: no entries match"));
            return Ok(());
        }
        let ordered = display_order(matches, args.filter.reverse);
        limited(ordered, args.last)
    } else {
        let mut all = Vec::new();
        for spec in &args.ids {
            all.extend(journal::resolve(&journal.entries, spec)?);
        }
        all.sort_unstable();
        all.dedup();
        all
    };

    for (n, index) in indices.iter().enumerate() {
        if n > 0 {
            say("");
        }
        render::print_raw(&render_entry(
            &journal.entries[*index],
            journal::entry_number(total, *index),
            paint,
            width,
            wrap,
        ));
    }
    Ok(())
}

fn cmd_list(cli: &Cli, cfg: &Config, paint: &Paint, args: &ListArgs) -> Result<()> {
    let journal = open_journal(cli, cfg)?;
    if journal.is_empty() {
        say(&paint.dim("tusk: the journal is empty — write something with `tusk new`"));
        return Ok(());
    }
    let query = full_query(&args.filter, Vec::new(), false, false, false)?;
    let matches = matching(&journal, &query);
    let total = journal.entries.len();
    if matches.is_empty() {
        say(&paint.dim("tusk: no entries match"));
        return Ok(());
    }
    let width = render::term_width();
    let number_width = total.to_string().len();
    let matched_count = matches.len();
    let shown = limited(display_order(matches, args.filter.reverse), args.limit);
    let shown_count = shown.len();

    let mut last_month: Option<String> = None;
    for index in &shown {
        let entry = &journal.entries[*index];
        if args.group {
            let month = entry.date.format("%Y-%m").to_string();
            if last_month.as_deref() != Some(month.as_str()) {
                let count = journal
                    .entries
                    .iter()
                    .filter(|e| e.date.format("%Y-%m").to_string() == month)
                    .count();
                if last_month.is_some() {
                    say("");
                }
                say(&render::month_heading(&month, count, paint, width));
                last_month = Some(month);
            }
        }
        say(&render::list_line(
            entry,
            journal::entry_number(total, *index),
            paint,
            None,
            width,
            number_width,
            cli.verbose > 0,
        ));
    }
    say("");
    say(&paint.dim(&format!(
        "{}  ·  1 = newest, {} = oldest",
        describe_limit(shown_count, matched_count),
        total
    )));
    Ok(())
}

fn render_entry(entry: &Entry, number: usize, paint: &Paint, width: usize, wrap: bool) -> String {
    if wrap {
        render::prose_entry(entry, number, paint, width)
    } else {
        render::entry_block(entry, number, paint, None, width)
    }
}

/// Wrapping helps a person read and hurts a script, so only wrap on a tty.
fn wrap_output() -> bool {
    std::io::stdout().is_terminal()
}

/// The prose width to use, narrowed to fit the terminal.
fn output_width(cfg: &Config, override_width: Option<usize>) -> usize {
    let preferred = override_width.unwrap_or_else(|| cfg.prose_width());
    if std::io::stdout().is_terminal()
        && let Ok((cols, _)) = ui::frame::size()
    {
        return ui::Prose::fit(cols, preferred).width;
    }
    preferred
}

fn cmd_read(cli: &Cli, cfg: &Config, args: &ReadArgs) -> Result<()> {
    let paint = Paint::new(cli.no_color);
    let mut journal = open_journal(cli, cfg)?;
    if journal.is_empty() {
        note(&paint.dim("tusk: the journal is empty — write something with `tusk new`"));
        return Ok(());
    }

    if args.raw {
        if args.pager {
            return editor::page(&journal.content, cfg);
        }
        render::print_raw(&journal.content);
        return Ok(());
    }

    let width = output_width(cfg, args.width);
    let query = full_query(&args.filter, Vec::new(), false, false, false)?;
    let mut indices: Vec<usize> = if args.ids.is_empty() {
        matching(&journal, &query)
    } else {
        let mut all = Vec::new();
        for spec in &args.ids {
            all.extend(journal::resolve(&journal.entries, spec)?);
        }
        all.sort_unstable();
        all.dedup();
        all
    };
    if indices.is_empty() {
        say(&paint.dim("tusk: no entries match"));
        return Ok(());
    }

    // The reader needs a terminal; otherwise print wrapped prose, or hand the
    // text to the user's own pager if they asked for one.
    let use_reader = ui::frame::interactive() && !args.print && !args.pager;
    if !use_reader {
        let text = plain_prose(&journal, &indices, width, &paint);
        if args.pager {
            return editor::page(&text, cfg);
        }
        render::print_raw(&text);
        return Ok(());
    }

    let mut follow = None;
    let mut spell = ui::spell::Spell::new(cfg);
    loop {
        let doc = build_doc(&journal, &indices);
        let options = ui::reader::ReaderOptions {
            title: format!("tusk read · {}", display_path(&journal.path)),
            prose_width: width,
            follow_entry: follow,
        };
        match ui::reader::run(&doc, &options, &paint, &mut spell)? {
            ui::reader::Action::Quit => break,
            ui::reader::Action::Edit(index) => {
                let entry = journal.entries[index].clone();
                let number = journal::entry_number(journal.entries.len(), index);
                let title = format!(
                    "editing entry {number}  ·  {}",
                    entry.date.format(journal::STAMP_SHORT)
                );
                let draft = Draft {
                    date: entry.date,
                    editing: Some(index),
                    title: &title,
                    editor: &args.editor,
                };
                compose(cli, cfg, &paint, &mut journal, &draft)?;
                follow = Some(index);
                // A replaced entry keeps its place, so the index list holds.
                indices.retain(|i| *i < journal.entries.len());
            }
        }
    }
    Ok(())
}

/// Build the reader's document from a list of entry indices, oldest first.
fn build_doc(journal: &Journal, indices: &[usize]) -> Doc {
    let total = journal.entries.len();
    let mut doc = Doc::new();
    for (n, index) in indices.iter().enumerate() {
        if n > 0 {
            doc.blank();
        }
        let entry = &journal.entries[*index];
        let number = journal::entry_number(total, *index);
        doc.heading(
            format!(
                "{}  ·  entry {}",
                entry.date.format(journal::STAMP_SHORT),
                number
            ),
            *index,
        );
        if entry.body.is_empty() {
            doc.body("(empty)", *index);
        } else {
            for line in entry.body.lines() {
                doc.body(line, *index);
            }
        }
    }
    doc
}

/// Wrapped prose for printing rather than paging.
fn plain_prose(journal: &Journal, indices: &[usize], width: usize, paint: &Paint) -> String {
    let total = journal.entries.len();
    let mut out = String::new();
    for (n, index) in indices.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        out.push_str(&render::prose_entry(
            &journal.entries[*index],
            journal::entry_number(total, *index),
            paint,
            width,
        ));
    }
    out
}

/// Shorten a path for a footer by replacing the home directory with `~`.
fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Ok(home) = config::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = text.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    text
}

fn cmd_edit(cli: &Cli, cfg: &Config, paint: &Paint, args: &EditArgs) -> Result<()> {
    let mut journal = open_journal(cli, cfg)?;
    let spec = args.id.clone().unwrap_or_else(|| "latest".to_string());
    let hits = journal::resolve(&journal.entries, &spec)?;
    if hits.len() > 1 {
        let newest = journal::entry_number(journal.entries.len(), *hits.last().expect("non-empty"));
        note(&paint.dim(&format!(
            "tusk: {spec} matches {} entries; editing the newest, entry {newest}",
            hits.len()
        )));
    }
    let index = *hits.last().expect("resolve is never empty");
    let entry = journal.entries[index].clone();
    let date = match &args.date {
        Some(spec) => journal::parse_datetime_spec(spec)?,
        None => entry.date,
    };
    let number = journal::entry_number(journal.entries.len(), index);
    let title = format!(
        "editing entry {number}  ·  {}",
        date.format(journal::STAMP_SHORT)
    );

    let draft = Draft {
        date,
        editing: Some(index),
        title: &title,
        editor: &args.editor,
    };
    match compose(cli, cfg, paint, &mut journal, &draft)? {
        Compose::Saved(index) => {
            let updated = journal.entries[index].clone();
            report_write(&journal, &updated, paint, "updated");
            Ok(())
        }
        Compose::Unchanged => {
            say(&paint.dim("tusk: no changes"));
            Ok(())
        }
    }
}

/// What came back from composing an entry.
enum Compose {
    /// Saved at least once; the index of the entry in the journal.
    Saved(usize),
    /// The user quit without changing anything.
    Unchanged,
}

/// Compose an entry in the built-in editor, or in an external one if asked.
///
/// Ctrl+S in the built-in editor saves through to the journal, so a long entry
/// can be committed early and then carry on being edited.
/// What is being composed: a new entry or an edit of an existing one.
struct Draft<'a> {
    date: NaiveDateTime,
    /// Index of the entry being edited, if this is an edit.
    editing: Option<usize>,
    title: &'a str,
    editor: &'a EditorOpts,
}

fn compose(
    cli: &Cli,
    cfg: &Config,
    paint: &Paint,
    journal: &mut Journal,
    draft: &Draft<'_>,
) -> Result<Compose> {
    let initial = match draft.editing {
        Some(index) => journal.entries[index].body.clone(),
        None => String::new(),
    };
    let choice = choose_editor(
        draft.editor,
        cfg,
        "run `tusk new` in a terminal, pipe the text in (`something | tusk new`),\n  \
         or set $TUSK_EDITOR",
    )?;

    match choice {
        EditorChoice::Builtin | EditorChoice::BuiltinRequested => {
            let mut buffer = Buffer::new(&initial);
            let options = ui::editor::EditorOptions {
                title: draft.title.to_string(),
                prose_width: cfg.prose_width(),
            };
            let mut saved_index = draft.editing;
            let mut saved_any = false;
            let backup = !cli.no_backup;
            let mut spell = ui::spell::Spell::new(cfg);
            {
                let mut saver = |text: &str| -> Result<()> {
                    let body = editor::clean_text(text);
                    if body.trim().is_empty() {
                        bail!("the entry would be empty");
                    }
                    if journal::looks_like_header(&body) {
                        note("tusk: warning: a line of this entry looks like an entry header");
                    }
                    match saved_index {
                        Some(index) => journal.replace(index, draft.date, &body),
                        None => {
                            journal.append(draft.date, &body);
                            saved_index = Some(journal.entries.len() - 1);
                        }
                    }
                    journal.save(backup)?;
                    saved_any = true;
                    Ok(())
                };
                ui::editor::run(&mut buffer, &options, paint, &mut saver, &mut spell)?;
            }
            match (saved_any, saved_index) {
                (true, Some(index)) => Ok(Compose::Saved(index)),
                _ => Ok(Compose::Unchanged),
            }
        }
        EditorChoice::External(command) => {
            let Some(text) = editor::edit_in_editor(
                &initial,
                if draft.editing.is_some() {
                    "edit"
                } else {
                    "new"
                },
                &command,
            )?
            else {
                return Ok(Compose::Unchanged);
            };
            if text.trim().is_empty() {
                bail!("refusing to save an empty entry — leave the entry as it was, or delete it");
            }
            if journal::looks_like_header(&text) {
                note("tusk: warning: a line of this entry looks like an entry header");
            }
            let index = match draft.editing {
                Some(index) => {
                    journal.replace(index, draft.date, &text);
                    index
                }
                None => {
                    journal.append(draft.date, &text);
                    journal.entries.len() - 1
                }
            };
            journal.save(!cli.no_backup)?;
            Ok(Compose::Saved(index))
        }
    }
}

fn cmd_delete(cli: &Cli, cfg: &Config, paint: &Paint, args: &DeleteArgs) -> Result<()> {
    let mut journal = open_journal(cli, cfg)?;
    let spec = args.id.clone().unwrap_or_else(|| "latest".to_string());
    let hits = journal::resolve(&journal.entries, &spec)?;
    let total = journal.entries.len();

    if hits.len() > 1 {
        let numbers: Vec<String> = hits
            .iter()
            .rev()
            .map(|i| journal::entry_number(total, *i).to_string())
            .collect();
        bail!(
            "`{spec}` matches {} entries ({}) — name a single one by number",
            hits.len(),
            numbers.join(", ")
        );
    }

    let index = hits[0];
    let entry = journal.entries[index].clone();
    let number = journal::entry_number(total, index);

    if args.dry_run {
        say(&render::entry_block(
            &entry,
            number,
            paint,
            None,
            render::term_width(),
        ));
        say(&paint.dim(&format!("tusk: would delete entry {number}")));
        return Ok(());
    }

    say(&render::entry_block(
        &entry,
        number,
        paint,
        None,
        render::term_width(),
    ));
    if !args.yes && !render::confirm(&format!("tusk: delete entry {number}?")) {
        say(&paint.dim("tusk: nothing deleted"));
        return Ok(());
    }

    journal.remove(index);
    journal.save(!cli.no_backup)?;
    say(&format!(
        "{} deleted entry {number} from {}",
        paint.ok("✓"),
        paint.dim(&journal.path.display().to_string())
    ));
    Ok(())
}

fn cmd_search(cli: &Cli, cfg: &Config, paint: &Paint, args: &SearchArgs) -> Result<()> {
    let journal = open_journal(cli, cfg)?;
    if journal.is_empty() {
        say(&paint.dim("tusk: the journal is empty — write something with `tusk new`"));
        std::process::exit(1);
    }
    if args.query.is_empty()
        && args.filter.tag.is_none()
        && args.filter.from.is_none()
        && args.filter.to.is_none()
    {
        bail!("give me something to search for, or use --tag / --from / --to");
    }

    let query = full_query(
        &args.filter,
        args.query.clone(),
        args.regex,
        args.case_sensitive,
        args.any,
    )?;
    let matches = matching(&journal, &query);

    if args.count {
        say(&matches.len().to_string());
        if matches.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }

    if matches.is_empty() {
        note(&paint.dim("tusk: no matches"));
        std::process::exit(1);
    }

    let total = journal.entries.len();
    let width = render::term_width();
    let number_width = total.to_string().len();
    let shown = limited(display_order(matches.clone(), false), args.limit);
    let highlight = query.highlight();

    if args.full {
        for (n, index) in shown.iter().enumerate() {
            if n > 0 {
                say("");
            }
            render::print_raw(&render::entry_block(
                &journal.entries[*index],
                journal::entry_number(total, *index),
                paint,
                highlight,
                width,
            ));
        }
    } else {
        for index in &shown {
            let entry = &journal.entries[*index];
            let number = journal::entry_number(total, *index);
            let occurrences = query.occurrences(entry);
            let extra = if occurrences > 1 {
                paint.dim(&format!("  +{}", occurrences - 1))
            } else {
                String::new()
            };
            let prefix = number_width + 2 + 16 + 2;
            let text = render::truncate(query.matching_line(entry), width.saturating_sub(prefix));
            say(&format!(
                "{}  {}  {}{}",
                paint.index(&format!("{number:>number_width$}")),
                paint.date(&entry.date.format(journal::STAMP_SHORT).to_string()),
                render::styled(&text, paint, highlight),
                extra
            ));
        }
    }

    say("");
    say(&paint.dim(&format!(
        "{}  ·  `tusk search --full …` to read them, `tusk show N` for one",
        describe_limit(shown.len(), matches.len())
    )));
    Ok(())
}

fn cmd_tags(cli: &Cli, cfg: &Config, paint: &Paint, args: &TagsArgs) -> Result<()> {
    let journal = open_journal(cli, cfg)?;
    let query = full_query(&args.filter, Vec::new(), false, false, false)?;
    let indices = matching(&journal, &query);
    if indices.is_empty() {
        say(&paint.dim("tusk: no entries match"));
        return Ok(());
    }

    let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for index in indices {
        for tag in journal.entries[index].tags() {
            let slot = counts
                .entry(tag.to_lowercase())
                .or_insert_with(|| (tag.clone(), 0));
            slot.1 += 1;
        }
    }
    if counts.is_empty() {
        say(&paint.dim("tusk: no tags yet — write #tags inside an entry"));
        return Ok(());
    }

    let mut tags: Vec<(String, usize)> = counts.into_values().collect();
    if args.alpha {
        tags.sort_by_key(|a| a.0.to_lowercase());
    } else {
        tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    let width = tags
        .iter()
        .map(|(_, c)| c.to_string().len())
        .max()
        .unwrap_or(1);
    let total = tags.len();
    let shown = if args.limit == 0 {
        tags
    } else {
        tags.into_iter().take(args.limit).collect()
    };
    for (tag, count) in &shown {
        say(&format!(
            "{}  {}",
            paint.index(&format!("{count:>width$}")),
            paint.tag(tag)
        ));
    }
    if shown.len() < total {
        say("");
        say(&paint.dim(&format!("{} of {total} tags", shown.len())));
    }
    Ok(())
}

fn cmd_stats(cli: &Cli, cfg: &Config, paint: &Paint, args: &StatsArgs) -> Result<()> {
    let journal = open_journal(cli, cfg)?;
    let query = full_query(&args.filter, Vec::new(), false, false, false)?;
    let indices = matching(&journal, &query);
    if indices.is_empty() {
        say(&paint.dim("tusk: no entries match"));
        return Ok(());
    }

    let entries: Vec<&Entry> = indices.iter().map(|i| &journal.entries[*i]).collect();
    let words: usize = entries.iter().map(|e| e.words()).sum();
    let first = entries.first().expect("non-empty").date;
    let last = entries.last().expect("non-empty").date;
    let span_days = (last.date() - first.date()).num_days().max(0) + 1;

    let mut dates: Vec<NaiveDate> = entries.iter().map(|e| e.date.date()).collect();
    dates.sort_unstable();
    dates.dedup();
    let (current_streak, longest_streak) = streaks(&dates);

    let mut months: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &entries {
        *months
            .entry(entry.date.format("%Y-%m").to_string())
            .or_insert(0) += 1;
    }
    let busiest =
        months.iter().fold(
            None,
            |best: Option<(&String, usize)>, (month, count)| match best {
                Some((_, best_count)) if *count < best_count => best,
                _ => Some((month, *count)),
            },
        );

    let longest = entries.iter().max_by_key(|e| e.words()).expect("non-empty");

    say(&render::stat_row(
        "journal",
        &journal.path.display().to_string(),
        paint,
    ));
    say(&render::stat_row(
        "entries",
        &format!(
            "{}{}",
            thousands(entries.len()),
            if entries.len() == journal.entries.len() {
                String::new()
            } else {
                format!(" of {}", thousands(journal.entries.len()))
            }
        ),
        paint,
    ));
    say(&render::stat_row(
        "words",
        &format!(
            "{}  ·  average {} per entry",
            thousands(words),
            thousands(if entries.is_empty() {
                0
            } else {
                words / entries.len()
            })
        ),
        paint,
    ));
    say(&render::stat_row(
        "longest",
        &format!(
            "{} words on {}",
            thousands(longest.words()),
            longest.date.format(journal::STAMP_SHORT)
        ),
        paint,
    ));
    say(&render::stat_row(
        "span",
        &format!(
            "{} → {}  ·  {} days, {} writing days",
            first.format("%Y-%m-%d"),
            last.format("%Y-%m-%d"),
            thousands(span_days as usize),
            thousands(dates.len())
        ),
        paint,
    ));
    say(&render::stat_row(
        "streak",
        &format!(
            "{} day{} now  ·  longest {}",
            current_streak,
            if current_streak == 1 { "" } else { "s" },
            longest_streak
        ),
        paint,
    ));
    if let Some((month, count)) = busiest {
        say(&render::stat_row(
            "busiest",
            &format!("{month}  ·  {count} entries"),
            paint,
        ));
    }
    let tags = journal.tag_counts();
    if !tags.is_empty() {
        let line = tags
            .iter()
            .take(6)
            .map(|(tag, count)| format!("{tag} {count}"))
            .collect::<Vec<_>>()
            .join("  ·  ");
        say(&render::stat_row("tags", &paint.tag(&line), paint));
    }
    Ok(())
}

/// `tusk open [PATH]` — the built-in editor, pointed at any text file.
///
/// With no path this is the journal file itself, as raw text, which is what
/// you want when an entry needs fixing by hand. With a path it is an ordinary
/// file: created on the first save if it does not exist yet.
fn cmd_open(cli: &Cli, cfg: &Config, paint: &Paint, args: &OpenArgs) -> Result<()> {
    let journal_path = config::resolve_path(cli.file.as_deref(), cfg)?;
    let path = match &args.path {
        Some(path) => config::expand(path),
        None => journal_path.clone(),
    };
    // The journal keeps its backups; any other file is just a file.
    let is_journal = same_file(&path, &journal_path);
    let label = display_path(&path);

    let exists = match fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => bail!("{label} is a directory, not a file"),
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err).with_context(|| format!("cannot read {label}")),
    };
    if !exists {
        // tusk will make the file, but not a directory tree behind your back.
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.is_dir() {
            bail!(
                "no such directory: {}\n  \
                 create it first, or name a file that already has a home",
                display_path(parent)
            );
        }
    }
    // Write through a symlink instead of replacing it with a regular file.
    let target = if exists {
        fs::canonicalize(&path).unwrap_or_else(|_| path.clone())
    } else {
        path.clone()
    };
    let text = if exists {
        fs::read_to_string(&target)
            .with_context(|| format!("cannot read {label} (tusk edits UTF-8 text files)"))?
    } else {
        String::new()
    };
    if text.contains('\0') {
        bail!("{label} looks like a binary file; tusk only edits text");
    }

    let command = choose_editor(
        &args.editor,
        cfg,
        &format!("run `tusk open {label}` in a terminal, or name an editor with `--editor`"),
    )?;

    match command {
        // An external editor edits the file where it lies; nothing to shuttle.
        EditorChoice::External(command) => {
            if is_journal && !cli.no_backup {
                // The editor writes the file itself, so the copy has to be
                // taken before it starts rather than before a save.
                back_up(&target, paint);
            }
            if !exists {
                write_text_atomic(&target, "")?;
            }
            let before = fs::read(&target).ok();
            editor::run_editor(&target, &command)?;
            let after = fs::read(&target).ok();
            if before == after {
                say(&paint.dim(&format!("tusk: {label} unchanged")));
            } else {
                report_file(&target, &label, paint, "wrote")?;
            }
            Ok(())
        }
        EditorChoice::Builtin | EditorChoice::BuiltinRequested => {
            let title = if exists {
                label.clone()
            } else {
                format!("{label}  ·  new file")
            };
            let options = ui::editor::EditorOptions {
                title,
                prose_width: cfg.prose_width(),
            };
            // A file tusk creates ends with a newline, as text files should.
            let new_file = !exists;
            let mut buffer = Buffer::new(&text);
            let mut spell = ui::spell::Spell::new(cfg);
            let mut saved: Option<(usize, usize)> = None;
            {
                let mut saver = |text: &str| -> Result<()> {
                    let body = finalise(text, new_file);
                    if is_journal && !cli.no_backup {
                        back_up(&target, paint);
                    }
                    write_text_atomic(&target, &body)?;
                    saved = Some((body.lines().count(), body.split_whitespace().count()));
                    Ok(())
                };
                ui::editor::run(&mut buffer, &options, paint, &mut saver, &mut spell)?;
            }
            match (saved, buffer.is_dirty()) {
                (Some((lines, words)), _) => say(&format!(
                    "{} {} {} ({}, {})",
                    paint.ok("✓"),
                    paint.bold("wrote"),
                    label,
                    count(lines, "line"),
                    count(words, "word")
                )),
                (None, true) => say(&paint.dim("tusk: nothing written")),
                (None, false) => say(&paint.dim(&format!("tusk: {label} unchanged"))),
            }
            Ok(())
        }
    }
}

/// Report a file an external editor just changed.
fn report_file(path: &Path, label: &str, paint: &Paint, verb: &str) -> Result<()> {
    let text = fs::read_to_string(path).unwrap_or_default();
    say(&format!(
        "{} {} {} ({}, {})",
        paint.ok("✓"),
        paint.bold(verb),
        label,
        count(text.lines().count(), "line"),
        count(text.split_whitespace().count(), "word")
    ));
    Ok(())
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Text is written as typed, except that a file tusk created ends with a
/// newline rather than stopping mid-line.
fn finalise(text: &str, new_file: bool) -> String {
    if new_file && !text.is_empty() && !text.ends_with('\n') {
        format!("{text}\n")
    } else {
        text.to_string()
    }
}

/// Write a text file the way the journal is written: to a sibling temporary
/// file, then rename over the original, so a crash cannot leave half a file.
/// Permissions of the file being replaced are carried over.
fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = journal::temp_sibling(path);
    let outcome = (|| -> Result<()> {
        fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    outcome
}

/// Keep a copy of the journal before it changes; a failure is a warning, not
/// a reason to lose the edit.
fn back_up(path: &Path, paint: &Paint) {
    if let Err(err) = journal::backup_file(path) {
        note(&paint.warn(&format!("tusk: warning: could not back up: {err:#}")));
    }
}

/// Whether two paths name the same file, following symlinks when they can.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b.to_path_buf(),
    }
}

fn cmd_path(cli: &Cli, cfg: &Config) -> Result<()> {
    let path = config::resolve_path(cli.file.as_deref(), cfg)?;
    say(&path.display().to_string());
    Ok(())
}

fn cmd_config(cli: &Cli, cfg: &Config, paint: &Paint, args: &ConfigArgs) -> Result<()> {
    let path = config::config_path()?;
    match args {
        ConfigArgs::Show => {
            say(&render::stat_row(
                "config",
                &format!(
                    "{} ({})",
                    path.display(),
                    if path.exists() {
                        "found"
                    } else {
                        "not created yet"
                    }
                ),
                paint,
            ));
            match config::resolve(cli.file.as_deref(), cfg) {
                Ok((journal, source)) => say(&render::stat_row(
                    "journal",
                    &format!(
                        "{}  {}",
                        journal.display(),
                        paint.dim(&format!("({})", source.describe()))
                    ),
                    paint,
                )),
                Err(_) => say(&render::stat_row(
                    "journal",
                    &paint.warn("unset — run `tusk init`"),
                    paint,
                )),
            }
            let editor = editor::choose(&EditorOpts::default(), cfg).describe();
            let editor_src = if std::env::var("TUSK_EDITOR").is_ok() {
                "from $TUSK_EDITOR"
            } else if cfg.editor.is_some() {
                "from config"
            } else {
                "default"
            };
            say(&render::stat_row(
                "editor",
                &format!("{editor}  ({editor_src})"),
                paint,
            ));
            say(&render::stat_row(
                "prose",
                &format!("{} columns", cfg.prose_width()),
                paint,
            ));
            match editor::pager_command(cfg) {
                Some(pager) => say(&render::stat_row("pager", &pager, paint)),
                None => say(&render::stat_row("pager", &paint.dim("none found"), paint)),
            }
            say(&render::stat_row(
                "backup",
                if cfg.backup { "on" } else { "off" },
                paint,
            ));
            // On demand: nothing is loaded until F2, so this is a probe.
            match crate::spell::Dictionary::load(cfg) {
                crate::spell::Status::Ready(dict) => {
                    let mut note = format!("on demand (F2)  ·  {}", dict.summary());
                    if let Some(source) = dict.source_note() {
                        note = format!("{note}  ·  {source}");
                    }
                    say(&render::stat_row("spellcheck", &note, paint));
                }
                crate::spell::Status::Disabled(_) => say(&render::stat_row(
                    "spellcheck",
                    &paint.warn("on demand (F2), but no dictionary found — see README"),
                    paint,
                )),
            }
            say("");
            match fs::read_to_string(&path) {
                Ok(text) => {
                    for line in text.lines() {
                        say(&paint.dim(line));
                    }
                }
                Err(_) => say(&paint.dim("(no config file yet)")),
            }
            Ok(())
        }
        ConfigArgs::Get { key } => {
            let key = normalize_key(key)?;
            let value = match key.as_str() {
                "journal" => config::resolve_path(cli.file.as_deref(), cfg)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                "editor" => editor::choose(&EditorOpts::default(), cfg).describe(),
                "pager" => editor::pager_command(cfg).unwrap_or_default(),
                "backup" => cfg.backup.to_string(),
                "prose_width" => cfg.prose_width().to_string(),
                "dictionary" => match std::env::var("TUSK_DICTIONARY") {
                    Ok(value) if !value.trim().is_empty() => {
                        format!("{value}  (from $TUSK_DICTIONARY)")
                    }
                    _ => cfg
                        .dictionary
                        .clone()
                        .unwrap_or_else(|| "en_GB (default)".to_string()),
                },
                "personal_words" => cfg
                    .personal_words
                    .clone()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(config dir)/words.txt".to_string()),
                _ => unreachable!("normalize_key validated the key"),
            };
            say(&value);
            Ok(())
        }
        ConfigArgs::Set { key, value } => {
            let key = normalize_key(key)?;
            let mut updated = cfg.clone();
            match key.as_str() {
                "journal" => {
                    let raw = value.trim();
                    if raw.is_empty() {
                        bail!("journal path cannot be empty");
                    }
                    let p =
                        config::expand(&PathBuf::from(raw.trim_matches('"').trim_matches('\'')));
                    updated.journal = Some(p);
                }
                "editor" => updated.editor = Some(value.clone()),
                "pager" => updated.pager = Some(value.clone()),
                "backup" => updated.backup = parse_bool(value)?,
                "prose_width" => updated.prose_width = parse_width(value)?,
                "dictionary" => updated.dictionary = Some(value.clone()),
                "personal_words" => {
                    updated.personal_words = Some(config::expand(PathBuf::from(value).as_path()))
                }
                _ => unreachable!("normalize_key validated the key"),
            }
            let written = config::save(&updated)?;
            say(&format!(
                "{} {key} = {value}  {}",
                paint.ok("✓"),
                paint.dim(&format!("({})", written.display()))
            ));
            Ok(())
        }
        ConfigArgs::Unset { key } => {
            let key = normalize_key(key)?;
            let mut updated = cfg.clone();
            match key.as_str() {
                "journal" => updated.journal = None,
                "editor" => updated.editor = None,
                "pager" => updated.pager = None,
                "backup" => updated.backup = true,
                "prose_width" => updated.prose_width = 80,
                "dictionary" => updated.dictionary = None,
                "personal_words" => updated.personal_words = None,
                _ => unreachable!("normalize_key validated the key"),
            }
            config::save(&updated)?;
            say(&format!("{} unset {key}", paint.ok("✓")));
            Ok(())
        }
        ConfigArgs::Edit => {
            if !path.exists() {
                config::save(cfg)?;
            }
            editor::run_editor(&path, &editor::editor_command(cfg))
        }
    }
}

fn cmd_init(cfg: &Config, paint: &Paint, args: &InitArgs) -> Result<()> {
    let detected_journal = args
        .file
        .clone()
        .or_else(|| cfg.journal.clone())
        .or_else(config::discover_jrnl)
        .unwrap_or_else(|| {
            config::home_dir()
                .map(|h| h.join("journal.txt"))
                .unwrap_or_else(|_| PathBuf::from("journal.txt"))
        });
    let detected_editor = cfg
        .editor
        .clone()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "builtin".to_string());

    let journal_path = if args.yes {
        detected_journal.clone()
    } else {
        PathBuf::from(render::prompt(
            "Journal file",
            &detected_journal.display().to_string(),
        ))
    };
    let journal_path = config::expand(&journal_path);

    let editor_value = if args.yes {
        args.editor.clone().unwrap_or(detected_editor)
    } else {
        render::prompt("Editor", args.editor.as_deref().unwrap_or(&detected_editor))
    };

    let mut updated = config::load();
    updated.journal = Some(journal_path.clone());
    if !editor_value.trim().is_empty() {
        updated.editor = Some(editor_value.clone());
    }
    let written = config::save(&updated)?;

    say(&format!(
        "{} configured tusk  {}",
        paint.ok("✓"),
        paint.dim(&format!("({})", written.display()))
    ));
    let existing = Journal::load(&journal_path)?;
    let mut facts = vec![format!("journal  {}", journal_path.display())];
    if existing.is_empty() {
        facts.push("0 entries so far".to_string());
        if !journal_path.exists() {
            facts.push("(the file will be created on your first entry)".to_string());
        }
    } else {
        let entries = existing.entries.len();
        let words = existing.total_words();
        let last = existing
            .entries
            .last()
            .map(|e| e.date.format(journal::STAMP_SHORT).to_string())
            .unwrap_or_default();
        facts.push(format!(
            "{entries} entries, {} words, latest {last}",
            thousands(words)
        ));
    }
    facts.push(format!("editor   {editor_value}"));
    match crate::spell::Dictionary::load(&updated) {
        crate::spell::Status::Ready(dict) => {
            facts.push(format!("spelling {} (F2 in the editor)", dict.summary()))
        }
        crate::spell::Status::Disabled(_) => {
            facts.push("spelling off — no dictionary found (see the README)".to_string())
        }
    }
    for fact in facts {
        say(&format!("  {}", paint.dim(&fact)));
    }
    say("");
    if editor::is_builtin_value(&editor_value) {
        say(&paint.dim("  tusk opens its own editor: type, Ctrl+S to save, Ctrl+Q to quit"));
    }
    say(&paint.dim("  try: tusk new · tusk read · tusk list · tusk search <word> · tusk edit"));
    Ok(())
}

// ---------------------------------------------------------------- utilities

fn normalize_key(key: &str) -> Result<String> {
    let key = key.trim().to_ascii_lowercase();
    let key = match key.as_str() {
        "file" | "path" | "journal" => "journal",
        "editor" => "editor",
        "pager" => "pager",
        "backup" | "backups" => "backup",
        "prose_width" | "prose" | "width" | "wrap" | "columns" => "prose_width",
        "dictionary" | "dict" | "language" | "lang" => "dictionary",
        "personal_words" | "words" | "wordlist" => "personal_words",
        other => bail!(
            "unknown setting `{other}` — try journal, editor, pager, backup, prose_width, \
             dictionary, or personal_words"
        ),
    };
    Ok(key.to_string())
}

fn parse_width(value: &str) -> Result<usize> {
    let width: usize = value
        .trim()
        .parse()
        .map_err(|_| anyhow!("`{value}` is not a number of columns"))?;
    if !(20..=200).contains(&width) {
        bail!("the prose width must be between 20 and 200 columns");
    }
    Ok(width)
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => bail!("`{other}` is not a yes/no value"),
    }
}

fn streaks(dates: &[NaiveDate]) -> (usize, usize) {
    let set: HashSet<NaiveDate> = dates.iter().copied().collect();
    let mut longest = 0usize;
    let mut run = 0usize;
    let mut previous: Option<NaiveDate> = None;
    for date in dates {
        run = match previous {
            Some(prev) if *date == prev + Duration::days(1) => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        previous = Some(*date);
    }

    let today = Local::now().date_naive();
    let mut cursor = if set.contains(&today) {
        Some(today)
    } else if set.contains(&(today - Duration::days(1))) {
        Some(today - Duration::days(1))
    } else {
        None
    };
    let mut current = 0usize;
    while let Some(day) = cursor {
        if set.contains(&day) {
            current += 1;
            cursor = Some(day - Duration::days(1));
        } else {
            break;
        }
    }
    (current, longest)
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_tusk_creates_ends_with_a_newline() {
        assert_eq!(finalise("hello", true), "hello\n");
        assert_eq!(finalise("hello\n", true), "hello\n");
        assert_eq!(finalise("", true), "");
        // An existing file is written back exactly as it was seen.
        assert_eq!(finalise("hello", false), "hello");
        assert_eq!(finalise("hello\n", false), "hello\n");
    }

    #[test]
    fn the_journal_is_recognised_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("tusk-same-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("journal.txt");
        fs::write(&file, "").unwrap();
        let link = dir.join("link.txt");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&file, &link).unwrap();

        assert!(same_file(&file, &file));
        assert!(same_file(&link, &file), "a symlink is the same file");
        assert!(!same_file(&file, &dir.join("other.txt")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_read_naturally() {
        assert_eq!(count(0, "line"), "0 lines");
        assert_eq!(count(1, "line"), "1 line");
        assert_eq!(count(2, "word"), "2 words");
    }

    #[test]
    fn formats_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(41_203), "41,203");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn computes_streaks() {
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 1, d).unwrap();
        let (current, longest) = streaks(&[day(1), day(2), day(3), day(9), day(10)]);
        assert_eq!(longest, 3);
        assert_eq!(current, 0);
    }

    #[test]
    fn validates_config_keys() {
        assert_eq!(normalize_key("file").unwrap(), "journal");
        assert_eq!(normalize_key("Backup").unwrap(), "backup");
        assert!(normalize_key("nonsense").is_err());
        assert!(parse_bool("on").unwrap());
        assert!(!parse_bool("off").unwrap());
        assert!(parse_bool("maybe").is_err());
    }
}
