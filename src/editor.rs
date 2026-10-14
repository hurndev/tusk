//! Talking to the outside world: your editor, your pager, temp files.

use std::env;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::config::Config;

/// Which editor to compose in: tusk's own, or something external.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorChoice {
    /// tusk's editor, chosen by default.
    Builtin,
    /// tusk's editor, explicitly asked for with a flag or setting.
    BuiltinRequested,
    External(String),
}

impl EditorChoice {
    pub fn describe(&self) -> String {
        match self {
            EditorChoice::Builtin | EditorChoice::BuiltinRequested => {
                "built-in (Ctrl+S saves)".to_string()
            }
            EditorChoice::External(command) => command.clone(),
        }
    }
}

fn is_builtin(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "builtin" | "built-in" | "tusk" | "internal" | "none"
    )
}

/// Is this config value naming tusk's own editor?
pub fn is_builtin_value(value: &str) -> bool {
    is_builtin(value)
}

/// An editor the user actually asked for: `$TUSK_EDITOR`, `$VISUAL`,
/// `$EDITOR`, or the config file. Values naming the built-in editor are
/// skipped.
fn explicit_editor(cfg: &Config) -> Option<String> {
    for var in ["TUSK_EDITOR", "VISUAL", "EDITOR"] {
        if let Ok(value) = env::var(var)
            && !value.trim().is_empty()
            && !is_builtin(&value)
        {
            return Some(value);
        }
    }
    if let Some(editor) = &cfg.editor
        && !editor.trim().is_empty()
        && !is_builtin(editor)
    {
        return Some(editor.clone());
    }
    None
}

/// An explicitly requested editor, if any. Used when the built-in editor cannot
/// run — for example with no terminal — where silently launching whatever an
/// unrelated config file mentions would be a good way to hang.
pub fn explicit_editor_command(cfg: &Config) -> Option<String> {
    explicit_editor(cfg)
}

/// Any configured external editor, including one inherited from jrnl.
fn external_editor(cfg: &Config) -> Option<String> {
    explicit_editor(cfg).or_else(crate::config::discover_jrnl_editor)
}

/// The external editor to use, falling back to `vi`.
pub fn editor_command(cfg: &Config) -> String {
    editor_command_opt(cfg).unwrap_or_else(|| "vi".to_string())
}

/// The external editor to use, if one is actually configured.
pub fn editor_command_opt(cfg: &Config) -> Option<String> {
    external_editor(cfg)
}

/// Decide, in order: `--editor`, `--external`, `--builtin`, `$TUSK_EDITOR`,
/// the config file, then tusk's own editor.
///
/// Note that a plain `$EDITOR`/`$VISUAL` does not opt you out of the built-in
/// editor; it is only consulted when you ask for an external one.
pub fn choose(flags: &crate::cli::EditorOpts, cfg: &Config) -> EditorChoice {
    if let Some(command) = &flags.editor_cmd {
        return if is_builtin(command) {
            EditorChoice::BuiltinRequested
        } else {
            EditorChoice::External(command.clone())
        };
    }
    if flags.external {
        return EditorChoice::External(editor_command(cfg));
    }
    if flags.builtin {
        return EditorChoice::BuiltinRequested;
    }
    if let Ok(value) = env::var("TUSK_EDITOR")
        && !value.trim().is_empty()
    {
        return if is_builtin(&value) {
            EditorChoice::BuiltinRequested
        } else {
            EditorChoice::External(value)
        };
    }
    if let Some(value) = &cfg.editor
        && !value.trim().is_empty()
    {
        return if is_builtin(value) {
            EditorChoice::BuiltinRequested
        } else {
            EditorChoice::External(value.clone())
        };
    }
    EditorChoice::Builtin
}

/// Which pager to use: `$TUSK_PAGER`, `$PAGER`, config, then `less` if present.
pub fn pager_command(cfg: &Config) -> Option<String> {
    for var in ["TUSK_PAGER", "PAGER"] {
        if let Ok(value) = env::var(var)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
    }
    if let Some(pager) = &cfg.pager
        && !pager.trim().is_empty()
    {
        return Some(pager.clone());
    }
    if in_path("less") {
        Some("less -R".to_string())
    } else {
        None
    }
}

fn in_path(binary: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(binary).is_file()))
        .unwrap_or(false)
}

/// A temporary file that deletes itself.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub fn new(tag: &str) -> Result<Scratch> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut path = env::temp_dir();
        path.push(format!("tusk-{tag}-{}-{nanos}.txt", std::process::id()));
        fs::write(&path, "").with_context(|| format!("cannot create {}", path.display()))?;
        Ok(Scratch { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<String> {
        fs::read_to_string(&self.path)
            .with_context(|| format!("cannot read {}", self.path.display()))
    }

    pub fn write(&self, text: &str) -> Result<()> {
        fs::write(&self.path, text).with_context(|| format!("cannot write {}", self.path.display()))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Open a file in an editor command and wait for it to close.
pub fn run_editor(path: &Path, command: &str) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{command} \"$1\""))
        .arg("tusk")
        .arg(path)
        .status()
        .with_context(|| format!("cannot launch editor `{command}`"))?;
    if !status.success() {
        bail!("editor `{command}` exited with {status}");
    }
    Ok(())
}

/// Compose or edit a block of text in an external editor.
///
/// Returns `None` when the buffer came back unchanged, so callers can skip
/// pointless writes.
pub fn edit_in_editor(initial: &str, tag: &str, command: &str) -> Result<Option<String>> {
    let scratch = Scratch::new(tag)?;
    scratch.write(initial)?;
    run_editor(scratch.path(), command)?;
    let edited = scratch.read()?;
    let cleaned = clean_text(&edited);
    if cleaned == clean_text(initial) {
        return Ok(None);
    }
    Ok(Some(cleaned))
}

/// Trim blank edges but keep interior blank lines exactly as written.
pub fn clean_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    normalized.trim_matches('\n').trim_end().to_string()
}

/// Send text through the pager when it is worth it.
pub fn page(text: &str, cfg: &Config) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if !std::io::stdout().is_terminal() {
        write_stdout(text);
        return Ok(());
    }
    let Some(pager) = pager_command(cfg) else {
        write_stdout(text);
        return Ok(());
    };
    let child = Command::new("sh")
        .arg("-c")
        .arg(&pager)
        .stdin(Stdio::piped())
        .spawn();
    match child {
        Ok(mut child) => {
            if let Some(stdin) = child.stdin.as_mut() {
                // A pager quitting early is normal, not an error.
                let _ = stdin.write_all(text.as_bytes());
            }
            drop(child.stdin.take());
            let _ = child.wait();
            Ok(())
        }
        Err(_) => {
            write_stdout(text);
            Ok(())
        }
    }
}

/// Write to stdout, quietly tolerating a closed pipe (`tusk list | head`).
pub fn write_stdout(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}
