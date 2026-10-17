//! The built-in prose editor: full-screen, modeless, soft-wrapped.
//!
//! You just type. Movement follows the wrapping, so Up and Down move by the
//! line you can see rather than by the line breaks in the file.

use std::collections::HashMap;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::frame::{self, Flash, Frame, Prose, Terminal};
use super::spell::Spell;
use crate::buffer::Buffer;
use crate::render::{self, Mark};
use crate::spell;
use crate::style::Paint;
use crate::text;

/// Blank screen lines kept above and below the cursor while scrolling.
const SCROLL_OFFSET: usize = 2;

pub struct EditorOptions {
    /// Shown in the status bar, e.g. "new entry" or "entry 12".
    pub title: String,
    /// Preferred prose width; narrowed to fit the terminal.
    pub prose_width: usize,
}

/// Run the editor over `buffer` until the user quits.
///
/// `save` is called every time the user presses Ctrl+S. It is handed the whole
/// buffer text and is expected to write it to the journal; returning an error
/// keeps the buffer marked modified and shows the message in the status bar.
pub fn run(
    buffer: &mut Buffer,
    options: &EditorOptions,
    paint: &Paint,
    save: &mut dyn FnMut(&str) -> Result<()>,
    spell: &mut Spell,
) -> Result<()> {
    let mut saved_once = false;
    let mut scroll = 0usize;
    let mut flash: Option<Flash> = None;
    let mut confirm_quit = false;

    {
        let _terminal = Terminal::enter()?;

        loop {
            let (cols, rows) = frame::size()?;
            let prose = Prose::fit(cols, options.prose_width);
            let layout = text::layout(&buffer.lines, prose.width);
            let view_height = rows.saturating_sub(2).max(1);
            let view = text::locate(&layout, &buffer.lines, buffer.row, buffer.col);

            // Keep the cursor on screen, with a little air around it.
            let air = SCROLL_OFFSET.min(view_height.saturating_sub(1) / 2);
            if view.display_row < scroll + air {
                scroll = view.display_row.saturating_sub(air);
            }
            if view.display_row + air >= scroll + view_height {
                scroll = (view.display_row + air + 1).saturating_sub(view_height);
            }
            let max_scroll = layout.lines.len().saturating_sub(view_height);
            scroll = scroll.min(max_scroll);
            let visible_end = (scroll + view_height).min(layout.lines.len());

            // Spellchecking looks at the lines on screen, and only once each,
            // however many display rows they occupy.
            let row_spans: HashMap<usize, Vec<(usize, usize)>> = match spell.active() {
                None => HashMap::new(),
                Some(dict) => {
                    let mut rows: Vec<usize> = layout.lines[scroll..visible_end]
                        .iter()
                        .map(|l| l.row)
                        .collect();
                    rows.dedup();
                    let mut found = HashMap::new();
                    for row in rows {
                        if let Some(line) = buffer.lines.get(row) {
                            let spans = dict.unknown_spans(line);
                            if !spans.is_empty() {
                                found.insert(row, spans);
                            }
                        }
                    }
                    found
                }
            };

            // The word under the cursor, if the dictionary does not know it.
            let unknown_here: Option<String> = match spell.active() {
                None => None,
                Some(dict) => spell::word_at(buffer.current_line(), buffer.col)
                    .map(|w| buffer.current_line()[w.bytes.0..w.bytes.1].to_string())
                    .filter(|word| dict.is_unknown(word)),
            };

            let mut frame = Frame::new(cols, rows);
            frame.clear();

            // The prose itself.
            for display_row in scroll..visible_end {
                let line = layout.lines[display_row];
                let chars: Vec<char> = buffer.lines[line.row].chars().collect();
                let end = line.end.max(line.start).min(chars.len());
                let raw: String = chars[line.start.min(end)..end].iter().collect();
                // Trimming here cannot move the marks: words never live in
                // trailing space.
                let body = raw.trim_end();

                let mut marks: Vec<(usize, usize, Mark)> = Vec::new();
                for (start, finish) in row_spans.get(&line.row).into_iter().flatten() {
                    // A word split across display rows is skipped rather than
                    // underlined twice.
                    if *start >= line.start && *finish <= line.end {
                        marks.push((*start - line.start, *finish - line.start, Mark::Misspelled));
                    }
                }
                for (start, finish) in render::tag_spans(body) {
                    marks.push((start, finish, Mark::Tag));
                }
                marks.sort_unstable_by_key(|(start, _, _)| *start);

                let styled = render::marked(body, paint, &marks);
                let styled = text::expand_tabs(&styled);
                let y = display_row - scroll;
                frame.put_raw(prose.margin, y, &styled);

                // Mark lines that continue because of soft wrapping.
                let continues = display_row < layout.per_row[line.row].1;
                if continues {
                    let marker_x = prose.margin + prose.width;
                    if marker_x < cols {
                        frame.put_raw(marker_x, y, &paint.dim("\u{21b5}"));
                    }
                }
            }

            // Hints, status, or a temporary message.
            let quit_prompt = if confirm_quit {
                Some(paint.warn(
                    "Unsaved changes — press y to discard and quit, any other key to keep writing",
                ))
            } else {
                None
            };
            let hint_y = rows.saturating_sub(2);
            let status_y = rows.saturating_sub(1);
            let spell_hint = match &unknown_here {
                Some(word) => format!("\"{word}\" isn't in the dictionary — F3 adds it"),
                None => "F2 spellcheck".to_string(),
            };
            let hint = match (&quit_prompt, &flash) {
                (Some(prompt), _) => prompt.clone(),
                (None, Some(flash)) => flash.render(paint),
                (None, None) => paint.dim(&format!(
                    "Ctrl+S save   Ctrl+Q quit   Ctrl+Z/Y undo/redo   Ctrl+K kill   {spell_hint}"
                )),
            };
            frame.put(0, hint_y, &hint);

            let state = if buffer.is_dirty() {
                paint.warn("modified")
            } else if saved_once {
                paint.ok("saved")
            } else {
                paint.dim("unchanged")
            };
            let status = format!(
                " {}   ·   {} words   ·   Ln {}, Col {}   ·   {} ",
                options.title,
                buffer.word_count(),
                buffer.row + 1,
                buffer.col + 1,
                state
            );
            let status = frame::pad(&status, cols);
            frame.put(0, status_y, &paint.reverse(&status));

            let cursor_x = prose.margin + view.x;
            frame.cursor(
                cursor_x,
                view.display_row.saturating_sub(scroll).min(view_height - 1),
            );
            frame.flush()?;

            match event::read()? {
                Event::Paste(pasted) => {
                    buffer.insert_str(&pasted);
                    flash = None;
                }
                Event::Resize(_, _) => {}
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if confirm_quit {
                        confirm_quit = false;
                        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                            break;
                        }
                        continue;
                    }

                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    let alt = key.modifiers.contains(KeyModifiers::ALT);
                    if !alt {
                        let mut session = Session {
                            buffer,
                            layout: &layout,
                            save,
                            spell,
                            saved_once: &mut saved_once,
                            view_height,
                        };
                        if let Some(outcome) = session.key(key.code, ctrl) {
                            match outcome {
                                Outcome::Message(message) => flash = Some(message),
                                Outcome::Quit => {
                                    if buffer.is_dirty() && !buffer.is_empty() {
                                        confirm_quit = true;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if flash.as_ref().is_some_and(Flash::expired) {
                        flash = None;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// What a keystroke wants to happen next.
enum Outcome {
    Message(Flash),
    Quit,
}

/// The state a keystroke may touch, bundled so the key handler can stay a
/// single match.
struct Session<'a> {
    buffer: &'a mut Buffer,
    layout: &'a text::Layout,
    save: &'a mut dyn FnMut(&str) -> Result<()>,
    spell: &'a mut Spell,
    saved_once: &'a mut bool,
    view_height: usize,
}

impl Session<'_> {
    fn key(&mut self, code: KeyCode, ctrl: bool) -> Option<Outcome> {
        let page = self.view_height as isize;
        match (code, ctrl) {
            (KeyCode::Char('s'), true) => {
                if self.buffer.is_empty() {
                    return Some(Outcome::Message(Flash::error(
                        "nothing to save yet — write something first",
                    )));
                }
                match (self.save)(&self.buffer.text()) {
                    Ok(()) => {
                        self.buffer.mark_saved();
                        *self.saved_once = true;
                        Some(Outcome::Message(Flash::new("saved")))
                    }
                    Err(err) => Some(Outcome::Message(Flash::error(format!(
                        "save failed: {err}"
                    )))),
                }
            }
            (KeyCode::Char('q'), true) | (KeyCode::Char('c'), true) => Some(Outcome::Quit),
            (KeyCode::Esc, _) => Some(Outcome::Quit),
            (KeyCode::F(2), _) => Some(Outcome::Message(self.spell.toggle())),
            (KeyCode::F(3), _) => {
                let line = self.buffer.current_line().to_string();
                let col = self.buffer.col;
                Some(Outcome::Message(self.spell.add_word(&line, col)))
            }
            (KeyCode::Char('z'), true) => {
                self.buffer.undo();
                None
            }
            (KeyCode::Char('y'), true) => {
                self.buffer.redo();
                None
            }
            (KeyCode::Char('k'), true) => {
                self.buffer.kill_to_end();
                None
            }
            (KeyCode::Char('u'), true) => {
                self.buffer.kill_to_start();
                None
            }
            (KeyCode::Char('a'), true) => {
                self.buffer.home();
                None
            }
            (KeyCode::Char('e'), true) => {
                self.buffer.end();
                None
            }
            (KeyCode::Char('w'), true) | (KeyCode::Backspace, true) => {
                self.buffer.delete_word_before();
                None
            }
            (KeyCode::Left, true) => {
                self.buffer.word_left();
                None
            }
            (KeyCode::Right, true) => {
                self.buffer.word_right();
                None
            }
            (KeyCode::Char(c), false) => {
                self.buffer.insert_char(c);
                None
            }
            (KeyCode::Enter, _) => {
                self.buffer.newline();
                None
            }
            (KeyCode::Tab, _) => {
                self.buffer.insert_str("    ");
                None
            }
            (KeyCode::Backspace, _) => {
                self.buffer.backspace();
                None
            }
            (KeyCode::Delete, _) => {
                self.buffer.delete();
                None
            }
            (KeyCode::Left, _) => {
                self.buffer.left();
                None
            }
            (KeyCode::Right, _) => {
                self.buffer.right();
                None
            }
            (KeyCode::Up, _) => {
                self.buffer.move_display(self.layout, -1);
                None
            }
            (KeyCode::Down, _) => {
                self.buffer.move_display(self.layout, 1);
                None
            }
            (KeyCode::PageUp, _) => {
                self.buffer.move_display(self.layout, -page);
                None
            }
            (KeyCode::PageDown, _) => {
                self.buffer.move_display(self.layout, page);
                None
            }
            (KeyCode::Home, true) => {
                self.buffer.doc_start();
                None
            }
            (KeyCode::End, true) => {
                self.buffer.doc_end();
                None
            }
            (KeyCode::Home, _) => {
                self.buffer.home();
                None
            }
            (KeyCode::End, _) => {
                self.buffer.end();
                None
            }
            _ => None,
        }
    }
}
