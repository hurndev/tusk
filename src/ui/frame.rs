//! Terminal plumbing for the built-in editor and reader.
//!
//! Everything here is best-effort about restoring the terminal: raw mode is
//! turned off and the alternate screen left on the way out, including when a
//! panic unwinds through the UI.

use std::io::{self, IsTerminal, Write};
use std::panic::{self, PanicHookInfo};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::style::Print;
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use crossterm::{execute, queue};

use crate::style::Paint;
use crate::text::{self, display_width};
/// How wide a prose block should be, and where it starts on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prose {
    /// Columns of text.
    pub width: usize,
    /// Blank columns to the left of the text.
    pub margin: usize,
}

impl Prose {
    /// Fit a preferred prose width into a terminal, leaving a margin.
    pub fn fit(term_width: usize, preferred: usize) -> Prose {
        let available = term_width.saturating_sub(2).max(1);
        let width = preferred.clamp(1, available.max(1)).min(available);
        let margin = term_width.saturating_sub(width) / 2;
        Prose { width, margin }
    }
}

/// A saved panic hook, so the terminal can be restored before the previous
/// hook prints anything.
type SavedHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// A terminal in raw mode, on the alternate screen, restored on drop.
pub struct Terminal {
    previous_hook: Option<SavedHook>,
}

impl Terminal {
    pub fn enter() -> Result<Terminal> {
        enable_raw_mode().context("cannot switch the terminal to raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide)
            .context("cannot take over the terminal")?;

        // Restore the terminal if we panic out of the UI.
        let previous = panic::take_hook();
        panic::set_hook(Box::new(|info| {
            let _ = restore();
            eprintln!("{info}");
        }));

        Ok(Terminal {
            previous_hook: Some(previous),
        })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = restore();
        if let Some(previous) = self.previous_hook.take() {
            panic::set_hook(previous);
        }
    }
}

fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, DisableBracketedPaste, Show, LeaveAlternateScreen)?;
    disable_raw_mode()
}

pub fn size() -> Result<(usize, usize)> {
    let (cols, rows) = terminal::size().context("cannot read the terminal size")?;
    Ok((cols.max(20) as usize, rows.max(3) as usize))
}

/// Whether we can run a full-screen UI at all.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// One frame, assembled in memory and written in a single syscall so the
/// screen never shows a half-drawn state.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    buffer: Vec<u8>,
    cursor: Option<(usize, usize)>,
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Frame {
        Frame {
            width,
            height,
            buffer: Vec::with_capacity(width * height * 2 + 64),
            cursor: None,
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        let _ = queue!(self.buffer, Clear(ClearType::All));
    }

    /// Draw text at a position, clipping it to the terminal width.
    pub fn put(&mut self, x: usize, y: usize, text: &str) {
        if y >= self.height || x >= self.width {
            return;
        }
        let clipped = clip(text, self.width - x);
        if clipped.is_empty() {
            return;
        }
        let _ = queue!(self.buffer, MoveTo(x as u16, y as u16), Print(clipped));
    }

    /// Draw text at a position without clipping (for pre-measured rows).
    pub fn put_raw(&mut self, x: usize, y: usize, text: &str) {
        if y >= self.height || x >= self.width {
            return;
        }
        let _ = queue!(self.buffer, MoveTo(x as u16, y as u16), Print(text));
    }

    pub fn cursor(&mut self, x: usize, y: usize) {
        self.cursor = Some((x.min(self.width.saturating_sub(1)), y));
    }

    pub fn flush(mut self) -> io::Result<()> {
        // The cursor is hidden unless a frame asks for it, so the editor and
        // reader only show one while typing.
        match self.cursor {
            Some((x, y)) => {
                let _ = queue!(self.buffer, Show, MoveTo(x as u16, y as u16));
            }
            None => {
                let _ = queue!(self.buffer, Hide);
            }
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(&self.buffer)?;
        stdout.flush()
    }
}

/// Pad plain text with spaces to fill a width, for status bars and headings.
pub fn pad(text: &str, width: usize) -> String {
    let visible = display_width(text);
    if visible >= width {
        return clip(text, width);
    }
    format!("{text}{}", " ".repeat(width - visible))
}

/// Clip a string to a number of terminal cells, ignoring nothing: the caller
/// passes plain text, and styling is applied afterwards.
pub fn clip(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in text.chars() {
        let w = text::char_width(c);
        if used + w > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// A transient status message with an expiry, used for "saved" confirmations.
#[derive(Debug, Clone)]
pub struct Flash {
    text: String,
    until: Instant,
    error: bool,
}

impl Flash {
    pub fn new(text: impl Into<String>) -> Flash {
        Flash {
            text: text.into(),
            until: Instant::now() + Duration::from_secs(3),
            error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Flash {
        Flash {
            text: text.into(),
            until: Instant::now() + Duration::from_secs(6),
            error: true,
        }
    }

    pub fn expired(&self) -> bool {
        Instant::now() > self.until
    }

    pub fn render(&self, paint: &Paint) -> String {
        if self.error {
            paint.warn(&self.text)
        } else {
            paint.ok(&self.text)
        }
    }
}
