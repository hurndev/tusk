//! Editing an ordinary text file in tusk's editor.
//!
//! `tuskedit` is a command-line front end for this module; `tusk open` uses it
//! for the journal file itself. Everything here is about the file rather than
//! the journal: atomic writes, kept permissions, and refusing what cannot be
//! edited safely.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::buffer::Buffer;
use crate::cli::EditorOpts;
use crate::config::{self, Config};
use crate::editor::{self, EditorChoice};
use crate::journal;
use crate::render::{note, say};
use crate::style::Paint;
use crate::ui;

/// Who is asking, and how they want to edit.
pub struct Options<'a> {
    /// The program name, for messages: "tusk" or "tuskedit".
    pub program: &'a str,
    pub no_backup: bool,
    /// The journal, when one is configured: it gets a backup before changing.
    pub journal: Option<&'a Path>,
    pub editor: Pick<'a>,
}

/// Which editor to open the file with.
pub enum Pick<'a> {
    /// Always tusk's own editor, whatever the config says (`tuskedit`).
    Builtin,
    /// Whatever the config, `$TUSK_EDITOR` or the flags ask for (`tusk open`).
    Configured(&'a EditorOpts),
}

/// Edit one file, creating it on the first save if it does not exist yet.
pub fn edit(path: &Path, cfg: &Config, paint: &Paint, options: &Options<'_>) -> Result<()> {
    let program = options.program;
    let path = config::expand(path);
    let is_journal = options
        .journal
        .is_some_and(|journal| same_file(&path, journal));
    let label = config::display_path(&path);

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
                config::display_path(parent)
            );
        }
    }
    let text = if exists {
        fs::read_to_string(&path)
            .with_context(|| format!("cannot read {label} (tusk edits UTF-8 text files)"))?
    } else {
        String::new()
    };
    if text.contains('\0') {
        bail!("{label} looks like a binary file; tusk only edits text");
    }
    // Write through a symlink instead of replacing it with a regular file.
    let target = if exists {
        fs::canonicalize(&path).unwrap_or_else(|_| path.clone())
    } else {
        path.clone()
    };

    let hint = format!("run `{program} {label}` in a terminal");
    let choice = match options.editor {
        Pick::Builtin => {
            if !ui::frame::interactive() {
                bail!("the built-in editor needs a terminal\n  {hint}");
            }
            EditorChoice::Builtin
        }
        Pick::Configured(flags) => editor::resolve_choice(
            flags,
            cfg,
            &format!("{hint}, or name an editor with `--editor`"),
        )?,
    };

    match choice {
        // An external editor edits the file where it lies; nothing to shuttle.
        EditorChoice::External(command) => {
            if is_journal && !options.no_backup {
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
                say(&paint.dim(&format!("{program}: {label} unchanged")));
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
            let editor_options = ui::editor::EditorOptions {
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
                    if is_journal && !options.no_backup {
                        back_up(&target, paint);
                    }
                    write_text_atomic(&target, &body)?;
                    saved = Some((body.lines().count(), body.split_whitespace().count()));
                    Ok(())
                };
                ui::editor::run(&mut buffer, &editor_options, paint, &mut saver, &mut spell)?;
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
                (None, true) => say(&paint.dim(&format!("{program}: nothing written"))),
                (None, false) => say(&paint.dim(&format!("{program}: {label} unchanged"))),
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
        _ => a == b,
    }
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
    fn counts_read_naturally() {
        assert_eq!(count(0, "line"), "0 lines");
        assert_eq!(count(1, "line"), "1 line");
        assert_eq!(count(2, "word"), "2 words");
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
    fn writing_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("tusk-mode-{}", std::process::id()));
        fs::write(&path, "before\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        write_text_atomic(&path, "after\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "the mode should have survived the rewrite");
        let _ = fs::remove_file(&path);
    }
}
