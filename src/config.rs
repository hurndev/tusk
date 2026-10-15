//! Configuration: a small TOML file, plus environment overrides and
//! one-piece-of-magic discovery of an existing `jrnl` setup.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Path to the journal file.
    pub journal: Option<PathBuf>,
    /// Editor used to compose and edit entries.
    pub editor: Option<String>,
    /// Pager used for long output.
    pub pager: Option<String>,
    /// Write a backup before modifying the journal.
    pub backup: bool,
    /// How wide prose should be in the editor and reader.
    pub prose_width: usize,
    /// Underline words the dictionary does not know, while editing.
    pub spellcheck: bool,
    /// A language tag such as `en_GB`, or a path to a word list.
    pub dictionary: Option<String>,
    /// Words to accept on top of the dictionary. Default: `words.txt` beside
    /// the config file.
    pub personal_words: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            journal: None,
            editor: None,
            pager: None,
            backup: true,
            prose_width: 80,
            spellcheck: true,
            dictionary: None,
            personal_words: None,
        }
    }
}

impl Config {
    /// The prose width, kept within a sane range whatever the file says.
    pub fn prose_width(&self) -> usize {
        self.prose_width.clamp(20, 200)
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(p) = env::var_os("TUSK_CONFIG").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if let Some(x) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(x).join("tusk").join("config.toml"));
    }
    Ok(home_dir()?.join(".config").join("tusk").join("config.toml"))
}

/// The directory tusk keeps its own files in (config, personal words).
pub fn config_dir() -> Result<PathBuf> {
    Ok(config_path()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".")))
}

pub fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .context("cannot determine your home directory (is $HOME set?)")
}

pub fn load() -> Config {
    let Ok(path) = config_path() else {
        return Config::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
            crate::render::note(&format!(
                "tusk: warning: {} is not valid config ({err}); using defaults",
                path.display()
            ));
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(cfg).context("cannot serialise config")?;
    fs::write(&path, format!("# tusk configuration\n{body}"))
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// Expand a leading `~` against `$HOME`.
pub fn expand(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~"
        && let Ok(home) = home_dir()
    {
        return home;
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Ok(home) = home_dir()
    {
        return home.join(rest);
    }
    path.to_path_buf()
}

/// Where the journal path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env(&'static str),
    Config,
    Jrnl,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::Flag => "from --file",
            Source::Env(var) => var,
            Source::Config => "from config",
            Source::Jrnl => "from your jrnl config",
        }
    }
}

/// Where the journal lives, in order of authority:
/// `--file`, `$TUSK_FILE`/`$TUSK_JOURNAL`, config, an existing jrnl config.
pub fn resolve(cli_file: Option<&Path>, cfg: &Config) -> Result<(PathBuf, Source)> {
    if let Some(p) = cli_file {
        return Ok((expand(p), Source::Flag));
    }
    for var in ["TUSK_FILE", "TUSK_JOURNAL"] {
        if let Some(v) = env::var_os(var).filter(|v| !v.is_empty()) {
            let label = if var == "TUSK_FILE" {
                "from $TUSK_FILE"
            } else {
                "from $TUSK_JOURNAL"
            };
            return Ok((expand(Path::new(&v)), Source::Env(label)));
        }
    }
    if let Some(p) = &cfg.journal {
        return Ok((expand(p), Source::Config));
    }
    if let Some(p) = discover_jrnl() {
        return Ok((p, Source::Jrnl));
    }
    bail!(
        "no journal file configured\n  \
         run `tusk init` to set one up, or pass --file PATH\n  \
         (or set $TUSK_FILE)"
    )
}

pub fn resolve_path(cli_file: Option<&Path>, cfg: &Config) -> Result<PathBuf> {
    resolve(cli_file, cfg).map(|(path, _)| path)
}

/// Look for an existing jrnl installation and borrow its journal path.
pub fn discover_jrnl() -> Option<PathBuf> {
    let text = jrnl_config_text()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("journal:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(expand(Path::new(value)));
            }
        }
    }
    None
}

/// Borrow the editor setting from an existing jrnl config, if there is one.
pub fn discover_jrnl_editor() -> Option<String> {
    for line in jrnl_config_text()?.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("editor:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn jrnl_config_text() -> Option<String> {
    let path = env::var_os("TUSK_JRNL_CONFIG")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().ok().map(|h| h.join(".config/jrnl/jrnl.yaml")))?;
    fs::read_to_string(path).ok()
}
