//! The built-in reader: full-screen, soft-wrapped prose, scrollable, with
//! in-place search. It never writes to the journal — pressing `e` hands an
//! entry back to the caller, which opens the editor.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use regex::{Regex, RegexBuilder};

use super::frame::{self, Flash, Frame, Prose, Terminal};
use crate::render;
use crate::style::Paint;
use crate::text;

/// What a line of the document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An entry divider, carrying a label such as "2026-06-09 11:20 · entry 3".
    Heading,
    /// Prose from an entry.
    Body,
    /// A gap between entries.
    Blank,
}

#[derive(Debug, Clone)]
pub struct DocLine {
    pub kind: Kind,
    pub text: String,
    /// Index into the journal's entries, when this line belongs to one.
    pub entry: Option<usize>,
}

/// A document ready to be read.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    pub lines: Vec<DocLine>,
}

impl Doc {
    pub fn new() -> Doc {
        Doc { lines: Vec::new() }
    }

    pub fn heading(&mut self, label: impl Into<String>, entry: usize) {
        self.lines.push(DocLine {
            kind: Kind::Heading,
            text: label.into(),
            entry: Some(entry),
        });
    }

    pub fn body(&mut self, text: impl Into<String>, entry: usize) {
        self.lines.push(DocLine {
            kind: Kind::Body,
            text: text.into(),
            entry: Some(entry),
        });
    }

    pub fn blank(&mut self) {
        self.lines.push(DocLine {
            kind: Kind::Blank,
            text: String::new(),
            entry: None,
        });
    }
}

/// One wrapped row on screen.
struct Visual {
    kind: Kind,
    text: String,
    entry: Option<usize>,
}

pub struct ReaderOptions {
    /// Shown in the footer.
    pub title: String,
    pub prose_width: usize,
    /// Open the view at this entry, e.g. after editing it.
    pub follow_entry: Option<usize>,
}

pub enum Action {
    Quit,
    /// The user asked to edit the entry nearest the top of the view.
    Edit(usize),
}

pub fn run(doc: &Doc, options: &ReaderOptions, paint: &Paint) -> Result<Action> {
    if !frame::interactive() {
        // The caller should have checked, but never take over a pipe.
        return Ok(Action::Quit);
    }

    let mut visuals: Vec<Visual> = Vec::new();
    let mut built_width = 0usize;
    let mut top = 0usize;
    // Where to start: the newest entry, at the bottom, like a log that keeps
    // being written to. Placed once, after the first layout.
    let mut placed = false;
    let mut query: Option<String> = None;
    let mut regex: Option<Regex> = None;
    let mut matches: Vec<usize> = Vec::new();
    let mut match_cursor = 0usize;
    let mut search_input: Option<String> = None;
    let mut help = false;
    let mut flash: Option<Flash> = None;

    {
        let _terminal = Terminal::enter()?;

        loop {
            let (cols, rows) = frame::size()?;
            let prose = Prose::fit(cols, options.prose_width);
            let height = rows.saturating_sub(1).max(1);

            if built_width != prose.width || visuals.is_empty() {
                visuals = build(doc, prose.width);
                built_width = prose.width;
                // Recompute matches for the new wrapping.
                if let Some(re) = &regex {
                    matches = find_matches(&visuals, re);
                }
            }

            let max_top = visuals.len().saturating_sub(height);
            top = top.min(max_top);

            if !placed {
                placed = true;
                top = match options.follow_entry {
                    Some(entry) => visuals
                        .iter()
                        .position(|v| v.entry == Some(entry))
                        .map(|at| at.saturating_sub(1))
                        .unwrap_or(max_top),
                    // Start at the bottom: the latest entry.
                    None => max_top,
                };
            }

            let mut frame = Frame::new(cols, rows);
            frame.clear();

            for row in 0..height {
                let Some(visual) = visuals.get(top + row) else {
                    break;
                };
                let focused = match_cursor < matches.len() && matches[match_cursor] == top + row;
                let styled = render_visual(visual, paint, regex.as_ref(), focused, prose.width);
                frame.put_raw(prose.margin, row, &styled);
            }

            // Footer.
            let percent = if visuals.is_empty() {
                100
            } else {
                ((top + height).min(visuals.len()) * 100) / visuals.len()
            };
            let position = visuals
                .iter()
                .skip(top)
                .find_map(|v| v.entry)
                .map(|entry| format!("entry {}", entry + 1))
                .unwrap_or_default();
            let footer = if let Some(input) = &search_input {
                // The real cursor is placed just after this text.
                format!(" /{input} ")
            } else if let Some(query) = &query {
                format!(
                    " {}   ·   /{query}   ·   {}/{}   ·   n next, N previous   ·   q quit ",
                    options.title,
                    if matches.is_empty() {
                        0
                    } else {
                        match_cursor + 1
                    },
                    matches.len(),
                )
            } else {
                format!(
                    " {}   ·   {}%   ·   {}   ·   q quit   / search   e edit   ? keys ",
                    options.title, percent, position
                )
            };
            let footer = frame::pad(&footer, cols);
            frame.put(0, rows.saturating_sub(1), &paint.reverse(&footer));

            if let Some(flash) = &flash {
                frame.put(
                    0,
                    rows.saturating_sub(2),
                    &paint.reverse(&frame::pad(&flash.render(paint), cols.min(60))),
                );
            }

            if help {
                draw_help(&mut frame, cols, rows, paint);
            } else if search_input.is_some() {
                frame.cursor(
                    2 + search_input
                        .as_ref()
                        .map(|s| s.chars().count())
                        .unwrap_or(0),
                    rows.saturating_sub(1),
                );
            }
            frame.flush()?;

            if flash.as_ref().is_some_and(Flash::expired) {
                flash = None;
            }

            match event::read()? {
                Event::Resize(_, _) => {}
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    // --- the search prompt swallows most keys ---
                    if let Some(input) = search_input.as_mut() {
                        match key.code {
                            KeyCode::Esc => search_input = None,
                            KeyCode::Enter => {
                                let entered = input.trim().to_string();
                                let entered = (!entered.is_empty()).then_some(entered);
                                search_input = None;
                                match entered {
                                    None => {
                                        query = None;
                                        regex = None;
                                        matches.clear();
                                    }
                                    Some(text) => {
                                        let re = RegexBuilder::new(&regex::escape(&text))
                                            .case_insensitive(true)
                                            .build()
                                            .ok();
                                        regex = re;
                                        query = Some(text);
                                        matches = regex
                                            .as_ref()
                                            .map(|re| find_matches(&visuals, re))
                                            .unwrap_or_default();
                                        match_cursor = 0;
                                        if matches.is_empty() {
                                            flash = Some(Flash::error("no matches"));
                                        } else {
                                            jump_to_match(&mut top, &matches, 0, height);
                                        }
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                input.pop();
                            }
                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                                input.push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- the help overlay ---
                    if help
                        && !matches!(
                            key.code,
                            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
                        )
                    {
                        help = false;
                    }

                    match key.code {
                        KeyCode::Char('q') => return Ok(Action::Quit),
                        KeyCode::Esc => return Ok(Action::Quit),
                        KeyCode::Char('?') => help = !help,
                        KeyCode::Char('/') => {
                            search_input = Some(String::new());
                            help = false;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            if !matches.is_empty() {
                                match_cursor = if key.code == KeyCode::Char('n') {
                                    (match_cursor + 1) % matches.len()
                                } else {
                                    (match_cursor + matches.len() - 1) % matches.len()
                                };
                                jump_to_match(&mut top, &matches, match_cursor, height);
                            } else {
                                flash = Some(Flash::error("press / to search first"));
                            }
                        }
                        KeyCode::Char('e') => {
                            if let Some(entry) = visuals.iter().skip(top).find_map(|v| v.entry) {
                                return Ok(Action::Edit(entry));
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => top = (top + 1).min(max_top),
                        KeyCode::Char('k') | KeyCode::Up => top = top.saturating_sub(1),
                        KeyCode::Char(' ') | KeyCode::Char('f') | KeyCode::PageDown => {
                            top = (top + height).min(max_top)
                        }
                        KeyCode::Char('b') | KeyCode::PageUp => top = top.saturating_sub(height),
                        KeyCode::Char('d') => top = (top + height / 2).min(max_top),
                        KeyCode::Char('u') => top = top.saturating_sub(height / 2),
                        KeyCode::Char('g') | KeyCode::Home => top = 0,
                        KeyCode::Char('G') | KeyCode::End => top = max_top,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn build(doc: &Doc, width: usize) -> Vec<Visual> {
    let mut visuals = Vec::new();
    for line in &doc.lines {
        match line.kind {
            Kind::Blank => visuals.push(Visual {
                kind: Kind::Blank,
                text: String::new(),
                entry: line.entry,
            }),
            Kind::Heading => {
                // A label longer than the prose width is clipped rather than
                // allowed to spill outside the text block.
                let rule = frame::clip(&render::rule(&line.text, width), width);
                visuals.push(Visual {
                    kind: Kind::Heading,
                    text: rule,
                    entry: line.entry,
                });
            }
            Kind::Body => {
                for segment in text::wrap_line(&line.text, width) {
                    let chars: Vec<char> = line.text.chars().collect();
                    let piece: String = chars[segment.start..segment.end].iter().collect();
                    visuals.push(Visual {
                        kind: Kind::Body,
                        text: text::expand_tabs(&piece),
                        entry: line.entry,
                    });
                }
            }
        }
    }
    visuals
}

fn find_matches(visuals: &[Visual], regex: &Regex) -> Vec<usize> {
    visuals
        .iter()
        .enumerate()
        .filter(|(_, v)| v.kind == Kind::Body && regex.is_match(&v.text))
        .map(|(i, _)| i)
        .collect()
}

fn jump_to_match(top: &mut usize, matches: &[usize], index: usize, height: usize) {
    let Some(&line) = matches.get(index) else {
        return;
    };
    *top = line.saturating_sub(height / 3);
}

fn render_visual(
    visual: &Visual,
    paint: &Paint,
    regex: Option<&Regex>,
    focused: bool,
    width: usize,
) -> String {
    let plain = frame::clip(visual.text.trim_end(), width);
    match visual.kind {
        Kind::Blank => String::new(),
        Kind::Heading => paint.separator(&plain),
        Kind::Body => {
            // Highlight search hits on plain text; otherwise, colour tags.
            // Never both, so no escape sequence can be matched by accident.
            match regex {
                Some(re) if re.is_match(&plain) => re
                    .replace_all(&plain, |caps: &regex::Captures| {
                        if focused {
                            paint.focus(&caps[0])
                        } else {
                            paint.hit(&caps[0])
                        }
                    })
                    .into_owned(),
                _ => render::colorize_tags(&plain, paint),
            }
        }
    }
}

fn draw_help(frame: &mut Frame, cols: usize, rows: usize, paint: &Paint) {
    let keys: [(&str, &str); 12] = [
        ("j / ↓", "scroll down"),
        ("k / ↑", "scroll up"),
        ("space / f", "page down"),
        ("b", "page up"),
        ("d / u", "half page"),
        ("g / G", "oldest / newest"),
        ("/", "search entries"),
        ("n / N", "next / previous hit"),
        ("e", "edit the entry here"),
        ("?", "hide these keys"),
        ("q", "quit"),
        ("", ""),
    ];
    let inner = keys
        .iter()
        .map(|(key, what)| key.chars().count() + what.chars().count() + 3)
        .max()
        .unwrap_or(20);
    let box_width = (inner + 4).min(cols.saturating_sub(2)).max(10);
    let box_height = keys.len() + 2;
    let x = (cols.saturating_sub(box_width)) / 2;
    let y = (rows.saturating_sub(box_height + 1)) / 2;

    for (i, (key, what)) in keys.iter().enumerate() {
        let line = if key.is_empty() {
            String::new()
        } else {
            format!("  {key:<10} {what}")
        };
        frame.put_raw(x, y + i, &paint.reverse(&frame::pad(&line, box_width)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Doc {
        let mut doc = Doc::new();
        doc.heading("2026-01-01 09:00 · entry 1", 0);
        doc.body("the quick brown fox jumps over the lazy dog", 0);
        doc.blank();
        doc.heading("2026-01-02 09:00 · entry 2", 1);
        doc.body("nothing to see here", 1);
        doc
    }

    #[test]
    fn wraps_document_lines_to_the_width() {
        let visuals = build(&doc(), 20);
        // heading, 3 body rows for the wrapped first entry, blank, heading, body
        for visual in &visuals {
            assert!(
                text::display_width(&visual.text) <= 20,
                "{:?} is too wide",
                visual.text
            );
        }
        let body_rows = visuals.iter().filter(|v| v.kind == Kind::Body).count();
        assert_eq!(body_rows, 4);
        assert_eq!(visuals[0].kind, Kind::Heading);
    }

    #[test]
    fn search_finds_body_rows_only() {
        let visuals = build(&doc(), 20);
        let re = RegexBuilder::new(&regex::escape("quick"))
            .case_insensitive(true)
            .build()
            .unwrap();
        let hits = find_matches(&visuals, &re);
        assert_eq!(hits.len(), 1);
        assert!(visuals[hits[0]].text.contains("quick"));

        // Dates live in headings, which are not searched.
        let date = RegexBuilder::new("2026-01-02").build().unwrap();
        assert!(find_matches(&visuals, &date).is_empty());

        // Case-insensitive.
        let upper = RegexBuilder::new(&regex::escape("NOTHING"))
            .case_insensitive(true)
            .build()
            .unwrap();
        assert_eq!(find_matches(&visuals, &upper).len(), 1);
    }
}
