//! The built-in prose editor: full-screen, modeless, soft-wrapped.
//!
//! You just type. Movement follows the wrapping, so Up and Down move by the
//! line you can see rather than by the line breaks in the file.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::frame::{self, Flash, Frame, Prose, Terminal};
use crate::buffer::Buffer;
use crate::render;
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

            let mut frame = Frame::new(cols, rows);
            frame.clear();

            // The prose itself.
            let visible_end = (scroll + view_height).min(layout.lines.len());
            for display_row in scroll..visible_end {
                let line = layout.lines[display_row];
                let chars: Vec<char> = buffer.lines[line.row].chars().collect();
                let raw: String = chars[line.start..line.start.max(line.end)].iter().collect();
                let body = text::expand_tabs(&raw);
                let body = body.trim_end();
                let styled = render::colorize_tags(body, paint);
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
            let hint = match (&quit_prompt, &flash) {
                (Some(prompt), _) => prompt.clone(),
                (None, Some(flash)) => flash.render(paint),
                (None, None) => paint.dim(
                    "Ctrl+S save   Ctrl+Q quit   Ctrl+Z undo   Ctrl+Y redo   \
                     Ctrl+K kill line   Ctrl+←/→ word",
                ),
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
                    if !alt
                        && let Some(flash_message) = handle_key(
                            buffer,
                            &layout,
                            key.code,
                            ctrl,
                            view_height,
                            save,
                            &mut saved_once,
                        )
                    {
                        match flash_message {
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

#[allow(clippy::too_many_arguments)]
fn handle_key(
    buffer: &mut Buffer,
    layout: &text::Layout,
    code: KeyCode,
    ctrl: bool,
    view_height: usize,
    save: &mut dyn FnMut(&str) -> Result<()>,
    saved_once: &mut bool,
) -> Option<Outcome> {
    let page = view_height as isize;
    match (code, ctrl) {
        (KeyCode::Char('s'), true) => {
            if buffer.is_empty() {
                return Some(Outcome::Message(Flash::error(
                    "nothing to save yet — write something first",
                )));
            }
            match save(&buffer.text()) {
                Ok(()) => {
                    buffer.mark_saved();
                    *saved_once = true;
                    Some(Outcome::Message(Flash::new("saved")))
                }
                Err(err) => Some(Outcome::Message(Flash::error(format!(
                    "save failed: {err}"
                )))),
            }
        }
        (KeyCode::Char('q'), true) | (KeyCode::Char('c'), true) => Some(Outcome::Quit),
        (KeyCode::Esc, _) => Some(Outcome::Quit),
        (KeyCode::Char('z'), true) => {
            buffer.undo();
            None
        }
        (KeyCode::Char('y'), true) => {
            buffer.redo();
            None
        }
        (KeyCode::Char('k'), true) => {
            buffer.kill_to_end();
            None
        }
        (KeyCode::Char('u'), true) => {
            buffer.kill_to_start();
            None
        }
        (KeyCode::Char('a'), true) => {
            buffer.home();
            None
        }
        (KeyCode::Char('e'), true) => {
            buffer.end();
            None
        }
        (KeyCode::Char('w'), true) | (KeyCode::Backspace, true) => {
            buffer.delete_word_before();
            None
        }
        (KeyCode::Left, true) => {
            buffer.word_left();
            None
        }
        (KeyCode::Right, true) => {
            buffer.word_right();
            None
        }
        (KeyCode::Char(c), false) => {
            buffer.insert_char(c);
            None
        }
        (KeyCode::Enter, _) => {
            buffer.newline();
            None
        }
        (KeyCode::Tab, _) => {
            buffer.insert_str("    ");
            None
        }
        (KeyCode::Backspace, _) => {
            buffer.backspace();
            None
        }
        (KeyCode::Delete, _) => {
            buffer.delete();
            None
        }
        (KeyCode::Left, _) => {
            buffer.left();
            None
        }
        (KeyCode::Right, _) => {
            buffer.right();
            None
        }
        (KeyCode::Up, _) => {
            buffer.move_display(layout, -1);
            None
        }
        (KeyCode::Down, _) => {
            buffer.move_display(layout, 1);
            None
        }
        (KeyCode::PageUp, _) => {
            buffer.move_display(layout, -page);
            None
        }
        (KeyCode::PageDown, _) => {
            buffer.move_display(layout, page);
            None
        }
        (KeyCode::Home, true) => {
            buffer.doc_start();
            None
        }
        (KeyCode::End, true) => {
            buffer.doc_end();
            None
        }
        (KeyCode::Home, _) => {
            buffer.home();
            None
        }
        (KeyCode::End, _) => {
            buffer.end();
            None
        }
        _ => None,
    }
}
