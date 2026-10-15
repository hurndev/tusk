//! Turning entries into text a person wants to read.

use std::io::IsTerminal;

use regex::{Captures, Regex};

use crate::journal::{Entry, tag_regex};
use crate::style::Paint;

/// Print a line to stdout, tolerating a closed pipe.
pub fn say(text: &str) {
    crate::editor::write_stdout(&format!("{text}\n"));
}

/// Print an unprefixed block to stdout.
pub fn print_raw(text: &str) {
    crate::editor::write_stdout(text);
}

/// Print a message to stderr (warnings, progress, prompts).
pub fn note(text: &str) {
    eprintln!("{text}");
}

/// Ask a question on stderr and read the answer, `default` on empty input.
pub fn prompt(question: &str, default: &str) -> String {
    use std::io::Write;

    if !std::io::stdin().is_terminal() {
        return default.to_string();
    }
    let hint = if default.is_empty() {
        String::new()
    } else {
        format!(" [{default}]")
    };
    eprint!("{question}{hint}: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => default.to_string(),
        Ok(_) => {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => default.to_string(),
    }
}

/// Confirm a destructive action.
pub fn confirm(question: &str) -> bool {
    use std::io::Write;

    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{question} [y/N]: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// How wide the terminal is, as best we can tell.
pub fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|w| *w >= 20)
        .unwrap_or(100)
}

/// Truncate on character boundaries, marking elision with an ellipsis.
pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Colour a rendered line without disturbing its visible width.
pub fn styled(text: &str, paint: &Paint, highlight: Option<&Regex>) -> String {
    if let Some(re) = highlight {
        // A single pass, so nothing inside the escape codes can be re-matched.
        re.replace_all(text, |caps: &Captures| paint.hit(&caps[0]))
            .into_owned()
    } else {
        colorize_tags(text, paint)
    }
}

pub fn colorize_tags(text: &str, paint: &Paint) -> String {
    if !paint.enabled {
        return text.to_string();
    }
    tag_regex()
        .replace_all(text, |caps: &Captures| paint.tag(&caps[0]))
        .into_owned()
}

/// Character spans of the `#tags` and `@names` in a line.
pub fn tag_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for m in tag_regex().find_iter(text) {
        // Byte offsets from the regex, character indices for the renderer.
        let start = text[..m.start()].chars().count();
        let end = start + m.as_str().chars().count();
        out.push((start, end));
    }
    out
}

/// What a marked span should look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// A `#tag` or `@name`.
    Tag,
    /// A word the dictionary does not know.
    Misspelled,
}

/// Style disjoint character ranges of a line, leaving the rest alone.
///
/// Spans must be sorted; any that overlap something already written are
/// ignored, so a caller cannot mangle the line by assembling them loosely.
pub fn marked(text: &str, paint: &Paint, marks: &[(usize, usize, Mark)]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + marks.len() * 8);
    let mut pos = 0usize;
    for (start, end, mark) in marks {
        let (start, end) = (*start, (*end).min(chars.len()));
        if start < pos || end <= start {
            continue;
        }
        if start > pos {
            out.extend(&chars[pos..start]);
        }
        let piece: String = chars[start..end].iter().collect();
        out.push_str(&match mark {
            Mark::Tag => paint.tag(&piece),
            Mark::Misspelled => paint.misspell(&piece),
        });
        pos = end;
    }
    if pos < chars.len() {
        out.extend(&chars[pos..]);
    }
    out
}

fn dim_rule(label: &str, paint: &Paint, width: usize) -> String {
    paint.separator(&rule(label, width))
}

/// A `── label ────` divider, plain text, sized to a prose width.
pub fn rule(label: &str, width: usize) -> String {
    let visible = label.chars().count() + 5;
    let total = width.clamp(20, 200).max(visible + 4);
    format!("── {label} {}", "─".repeat(total - visible))
}

/// An entry rendered as wrapped prose: a divider, then soft-wrapped lines.
///
/// This is what you see when reading on a terminal; when output is piped,
/// callers print the unwrapped text instead so other tools see your bytes.
pub fn prose_entry(entry: &Entry, number: usize, paint: &Paint, width: usize) -> String {
    let label = format!(
        "{}  ·  entry {}",
        entry.date.format(crate::journal::STAMP_SHORT),
        number
    );
    let mut out = String::new();
    out.push_str(&dim_rule(&label, paint, width));
    out.push('\n');
    if entry.body.is_empty() {
        out.push_str(&paint.dim("(empty)"));
        out.push('\n');
        return out;
    }
    for line in entry.body.lines() {
        for segment in crate::text::wrap_line(line, width) {
            let chars: Vec<char> = line.chars().collect();
            let piece: String = chars[segment.start..segment.end].iter().collect();
            let piece = crate::text::expand_tabs(&piece);
            out.push_str(&colorize_tags(piece.trim_end(), paint));
            out.push('\n');
        }
    }
    out
}

/// A full entry, with its heading rule.
pub fn entry_block(
    entry: &Entry,
    number: usize,
    paint: &Paint,
    highlight: Option<&Regex>,
    width: usize,
) -> String {
    let label = format!(
        "{}  ·  entry {}",
        entry.date.format(crate::journal::STAMP_SHORT),
        number
    );
    let body = if entry.body.is_empty() {
        paint.dim("(empty)")
    } else {
        styled(&entry.body, paint, highlight)
    };
    format!("{}\n{body}\n", dim_rule(&label, paint, width))
}

/// One line of a listing: number, date, first line.
pub fn list_line(
    entry: &Entry,
    number: usize,
    paint: &Paint,
    highlight: Option<&Regex>,
    width: usize,
    number_width: usize,
    verbose: bool,
) -> String {
    let index = paint.index(&format!("{number:>number_width$}"));
    let date = entry.date.format(crate::journal::STAMP_SHORT).to_string();
    let mut prefix_width = number_width + 2 + date.len() + 2;
    let mut text = entry.first_line();
    if text.is_empty() {
        text = "(empty)".to_string();
    }
    if verbose {
        let stats = format!(" [{} words, {} lines]", entry.words(), entry.lines());
        prefix_width += stats.len();
        let shown = truncate(&text, width.saturating_sub(prefix_width));
        return format!(
            "{index}  {}  {}{}",
            paint.date(&date),
            styled(&shown, paint, highlight),
            paint.dim(&stats)
        );
    }
    let shown = truncate(&text, width.saturating_sub(prefix_width));
    format!(
        "{index}  {}  {}",
        paint.date(&date),
        styled(&shown, paint, highlight)
    )
}

/// A month heading for `tusk list --group`.
pub fn month_heading(month: &str, count: usize, paint: &Paint, width: usize) -> String {
    let label = if count == 1 {
        format!("{month}  ·  1 entry")
    } else {
        format!("{month}  ·  {count} entries")
    };
    let visible = label.chars().count() + 3;
    let total = width.clamp(20, 100).max(visible + 4);
    paint.separator(&format!("── {label} {}", "─".repeat(total - visible)))
}

/// Align a `label   value` row for stats output.
pub fn stat_row(label: &str, value: &str, paint: &Paint) -> String {
    format!("{}  {value}", paint.dim(&format!("{label:<10}")))
}
