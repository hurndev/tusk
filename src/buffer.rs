//! The text buffer behind the built-in editor: cursor, editing operations,
//! and undo. Deliberately free of any terminal code so it can be tested.

/// How many undo steps to keep.
const UNDO_LIMIT: usize = 400;

/// Consecutive edits of the same kind collapse into one undo step, so a burst
/// of typing undoes as a single thought rather than one keystroke at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Insert,
    Backspace,
    Delete,
}

#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

/// An editable block of prose. `col` is a character index, never a byte index,
/// so multi-byte text behaves.
#[derive(Debug, Clone)]
pub struct Buffer {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    /// Remembered column for vertical movement.
    goal: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    run: Option<Run>,
    dirty: bool,
}

impl Buffer {
    pub fn new(text: &str) -> Buffer {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
        if lines.len() == 1 && lines[0].is_empty() {
            lines.clear();
            lines.push(String::new());
        }
        Buffer {
            lines,
            row: 0,
            col: 0,
            goal: None,
            undo: Vec::new(),
            redo: Vec::new(),
            run: None,
            dirty: false,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    pub fn word_count(&self) -> usize {
        self.lines
            .iter()
            .map(|l| l.split_whitespace().count())
            .sum()
    }

    pub fn line(&self, row: usize) -> &str {
        self.lines.get(row).map(String::as_str).unwrap_or("")
    }

    pub fn current_line(&self) -> &str {
        self.line(self.row)
    }

    /// Character length of a line.
    fn len_of(&self, row: usize) -> usize {
        self.line(row).chars().count()
    }

    /// Byte offset of a character index within the current row.
    fn byte_at(&self, row: usize, col: usize) -> usize {
        self.line(row)
            .char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.line(row).len())
    }

    // ------------------------------------------------------------ history

    fn record(&mut self, kind: Option<Run>) {
        let coalesce = matches!(
            (self.run, kind),
            (Some(Run::Insert), Some(Run::Insert))
                | (Some(Run::Backspace), Some(Run::Backspace))
                | (Some(Run::Delete), Some(Run::Delete))
        );
        if !coalesce {
            self.undo.push(Snapshot {
                lines: self.lines.clone(),
                row: self.row,
                col: self.col,
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.run = kind;
    }

    /// Called after movement, so the next edit starts a fresh undo step.
    fn break_run(&mut self) {
        self.run = None;
    }

    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        });
        self.lines = snapshot.lines;
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.dirty = true;
        self.break_run();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        });
        self.lines = snapshot.lines;
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.dirty = true;
        self.break_run();
        true
    }

    // ------------------------------------------------------------ editing

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.newline();
            return;
        }
        if c == '\r' {
            return;
        }
        self.record(Some(Run::Insert));
        let at = self.byte_at(self.row, self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
        self.goal = None;
        self.dirty = true;
    }

    /// Insert a block of text, as from a paste.
    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.record(None);
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut parts = normalized.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                let at = self.byte_at(self.row, self.col);
                self.lines[self.row].insert_str(at, part);
                self.col += part.chars().count();
            }
            if parts.peek().is_some() {
                let at = self.byte_at(self.row, self.col);
                let tail = self.lines[self.row].split_off(at);
                self.lines.insert(self.row + 1, tail);
                self.row += 1;
                self.col = 0;
            }
        }
        self.goal = None;
        self.dirty = true;
    }

    pub fn newline(&mut self) {
        self.record(Some(Run::Insert));
        let at = self.byte_at(self.row, self.col);
        let tail = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
        self.goal = None;
        self.dirty = true;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.record(Some(Run::Backspace));
            let at = self.byte_at(self.row, self.col);
            let prev = self.byte_at(self.row, self.col - 1);
            self.lines[self.row].replace_range(prev..at, "");
            self.col -= 1;
        } else if self.row > 0 {
            self.record(None);
            let tail = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.len_of(self.row);
            self.lines[self.row].push_str(&tail);
        } else {
            return;
        }
        self.goal = None;
        self.dirty = true;
    }

    pub fn delete(&mut self) {
        if self.col < self.len_of(self.row) {
            self.record(Some(Run::Delete));
            let at = self.byte_at(self.row, self.col);
            let next = self.byte_at(self.row, self.col + 1);
            self.lines[self.row].replace_range(at..next, "");
        } else if self.row + 1 < self.lines.len() {
            self.record(None);
            let tail = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&tail);
        } else {
            return;
        }
        self.goal = None;
        self.dirty = true;
    }

    /// Delete from the cursor to the end of the line.
    pub fn kill_to_end(&mut self) {
        let len = self.len_of(self.row);
        if self.col >= len {
            self.delete();
            return;
        }
        self.record(None);
        let at = self.byte_at(self.row, self.col);
        self.lines[self.row].truncate(at);
        self.goal = None;
        self.dirty = true;
    }

    /// Delete from the start of the line to the cursor.
    pub fn kill_to_start(&mut self) {
        if self.col == 0 {
            return;
        }
        self.record(None);
        let at = self.byte_at(self.row, self.col);
        self.lines[self.row].replace_range(..at, "");
        self.col = 0;
        self.goal = None;
        self.dirty = true;
    }

    /// Delete the word before the cursor.
    pub fn delete_word_before(&mut self) {
        if self.col == 0 {
            self.backspace();
            return;
        }
        self.record(None);
        let chars: Vec<char> = self.current_line().chars().collect();
        let mut target = self.col;
        while target > 0 && chars[target - 1].is_whitespace() {
            target -= 1;
        }
        while target > 0 && !chars[target - 1].is_whitespace() {
            target -= 1;
        }
        let from = self.byte_at(self.row, target);
        let to = self.byte_at(self.row, self.col);
        self.lines[self.row].replace_range(from..to, "");
        self.col = target;
        self.goal = None;
        self.dirty = true;
    }

    // ----------------------------------------------------------- movement

    pub fn left(&mut self) {
        self.break_run();
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.len_of(self.row);
        }
        self.goal = None;
    }

    pub fn right(&mut self) {
        self.break_run();
        if self.col < self.len_of(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
        self.goal = None;
    }

    pub fn home(&mut self) {
        self.break_run();
        self.col = 0;
        self.goal = None;
    }

    pub fn end(&mut self) {
        self.break_run();
        self.col = self.len_of(self.row);
        self.goal = None;
    }

    pub fn word_left(&mut self) {
        self.break_run();
        if self.col == 0 {
            self.left();
            return;
        }
        let chars: Vec<char> = self.current_line().chars().collect();
        let mut target = self.col;
        while target > 0 && chars[target - 1].is_whitespace() {
            target -= 1;
        }
        while target > 0 && !chars[target - 1].is_whitespace() {
            target -= 1;
        }
        self.col = target;
        self.goal = None;
    }

    pub fn word_right(&mut self) {
        self.break_run();
        let chars: Vec<char> = self.current_line().chars().collect();
        let mut target = self.col;
        while target < chars.len() && !chars[target].is_whitespace() {
            target += 1;
        }
        while target < chars.len() && chars[target].is_whitespace() {
            target += 1;
        }
        self.col = target.min(chars.len());
        self.goal = None;
    }

    /// Move by a number of *screen* lines, following the soft wrapping.
    pub fn move_display(&mut self, layout: &crate::text::Layout, delta: isize) {
        let view = crate::text::locate(layout, &self.lines, self.row, self.col);
        // Keep the horizontal screen column steady across a run of movements.
        let goal_x = *self.goal.get_or_insert(view.x);
        let last = layout.lines.len().saturating_sub(1) as isize;
        let target = (view.display_row as isize + delta).clamp(0, last) as usize;
        let display = layout.lines[target];

        let chars: Vec<char> = self.line(display.row).chars().collect();
        let mut x = 0usize;
        let mut col = display.start;
        while col < display.end && x < goal_x {
            x += crate::text::char_width(chars[col]);
            col += 1;
        }
        // At a hard break the end of one display line is also the start of the
        // next, so step back until the cursor really lands on the target line.
        while col > display.start
            && crate::text::locate(layout, &self.lines, display.row, col).display_row != target
        {
            col -= 1;
        }
        self.row = display.row;
        self.col = col;
        self.break_run();
    }

    pub fn doc_start(&mut self) {
        self.break_run();
        self.row = 0;
        self.col = 0;
        self.goal = None;
    }

    pub fn doc_end(&mut self) {
        self.break_run();
        self.row = self.lines.len().saturating_sub(1);
        self.col = self.len_of(self.row);
        self.goal = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text;

    fn buffer(text: &str) -> Buffer {
        Buffer::new(text)
    }

    #[test]
    fn inserts_and_splits() {
        let mut buf = buffer("");
        for c in "hello world".chars() {
            buf.insert_char(c);
        }
        assert_eq!(buf.text(), "hello world");
        buf.home();
        buf.insert_char('>');
        assert_eq!(buf.text(), ">hello world");
        buf.end();
        buf.newline();
        buf.insert_str("second line");
        assert_eq!(buf.text(), ">hello world\nsecond line");
        assert!(buf.is_dirty());
    }

    #[test]
    fn handles_multibyte_text() {
        let mut buf = buffer("héllo wörld");
        buf.end();
        buf.insert_char('!');
        assert_eq!(buf.text(), "héllo wörld!");
        buf.home();
        buf.right();
        buf.right();
        assert_eq!(buf.col, 2);
        buf.backspace();
        assert_eq!(buf.text(), "hllo wörld!");

        // Emoji and CJK are single characters, not byte slices.
        let mut buf = buffer("日");
        buf.end();
        buf.insert_char('本');
        assert_eq!(buf.text(), "日本");
        buf.home();
        buf.right();
        buf.backspace();
        assert_eq!(buf.text(), "本");
    }

    #[test]
    fn backspace_joins_lines_and_delete_pulls_them_up() {
        let mut buf = buffer("one\ntwo");
        buf.end();
        let layout = text::layout(&buf.lines, 80);
        buf.move_display(&layout, 1);
        buf.home();
        buf.backspace();
        assert_eq!(buf.text(), "onetwo");
        assert_eq!(buf.row, 0);
        assert_eq!(buf.col, 3);

        let mut buf = buffer("one\ntwo");
        buf.end();
        buf.delete();
        assert_eq!(buf.text(), "onetwo");

        let mut buf = buffer("one");
        buf.home();
        buf.backspace(); // no-op at the very start
        assert_eq!(buf.text(), "one");
        buf.delete();
        buf.delete();
        buf.delete();
        buf.delete(); // no-op at the very end
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn inserts_pasted_blocks() {
        let mut buf = buffer("start: ");
        buf.end();
        buf.insert_str("a\r\nb\nc");
        assert_eq!(buf.text(), "start: a\nb\nc");
        assert_eq!(buf.row, 2);
        assert_eq!(buf.col, 1);

        let mut buf = buffer("");
        buf.insert_str("\n\n");
        assert_eq!(buf.lines.len(), 3);
    }

    #[test]
    fn vertical_movement_remembers_the_screen_column() {
        // A long wrapped paragraph followed by a short line.
        let mut buf = buffer("alpha beta gamma delta\nshort");
        let layout = text::layout(&buf.lines, 11);
        // "alpha beta" / "gamma delta" / "short"
        let on_screen = |buf: &Buffer| text::locate(&layout, &buf.lines, buf.row, buf.col);
        buf.doc_start();
        for _ in 0..7 {
            buf.right();
        }
        assert_eq!(
            on_screen(&buf),
            crate::text::CursorView {
                display_row: 0,
                x: 7
            }
        );

        buf.move_display(&layout, 1);
        assert_eq!(on_screen(&buf).display_row, 1);
        assert_eq!(on_screen(&buf).x, 7, "same screen column on the next line");

        buf.move_display(&layout, 1);
        assert_eq!((buf.row, buf.col), (1, 5), "clamped on the short line");

        buf.move_display(&layout, -2);
        assert_eq!(on_screen(&buf).x, 7, "the goal column is remembered");
        assert_eq!(on_screen(&buf).display_row, 0);
    }

    #[test]
    fn vertical_movement_stays_on_the_target_line_at_hard_breaks() {
        // Every ten characters is a hard break; moving down must not slip past
        // the display line it was asked for.
        let mut buf = buffer("abcdefghijabcdefghijxy");
        let layout = text::layout(&buf.lines, 10);
        assert_eq!(layout.lines.len(), 3);
        let display_row =
            |buf: &Buffer| text::locate(&layout, &buf.lines, buf.row, buf.col).display_row;
        buf.doc_start();
        for _ in 0..9 {
            buf.right();
        }
        assert_eq!(display_row(&buf), 0);

        buf.move_display(&layout, 1);
        assert_eq!(display_row(&buf), 1);
        buf.move_display(&layout, 1);
        assert_eq!(display_row(&buf), 2);
        buf.move_display(&layout, -1);
        assert_eq!(display_row(&buf), 1);
        buf.move_display(&layout, -1);
        assert_eq!(display_row(&buf), 0);
    }

    #[test]
    fn word_movement_and_deletion() {
        let mut buf = buffer("the quick brown fox");
        buf.end();
        buf.word_left();
        assert_eq!(buf.col, 16, "start of fox");
        buf.word_left();
        assert_eq!(buf.col, 10);
        buf.word_right();
        assert_eq!(buf.col, 16);
        buf.delete_word_before();
        assert_eq!(buf.text(), "the quick fox");

        buf.end();
        buf.kill_to_end();
        assert_eq!(buf.text(), "the quick fox");
        buf.kill_to_start();
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn undo_coalesces_typing_and_redo_replays_it() {
        let mut buf = buffer("");
        for c in "hello".chars() {
            buf.insert_char(c);
        }
        assert_eq!(buf.text(), "hello");
        assert!(buf.undo());
        assert_eq!(buf.text(), "", "one undo for the whole burst of typing");
        assert!(buf.redo());
        assert_eq!(buf.text(), "hello");

        // Movement splits undo groups.
        buf.end();
        buf.newline();
        buf.insert_str("second");
        assert_eq!(buf.text(), "hello\nsecond");
        buf.undo();
        assert_eq!(buf.text(), "hello\n");
        buf.undo();
        assert_eq!(buf.text(), "hello");
        buf.undo();
        assert_eq!(buf.text(), "");
        assert!(!buf.undo(), "nothing left to undo");
    }

    #[test]
    fn undo_restores_after_deletions() {
        let mut buf = buffer("keep me");
        buf.end();
        buf.kill_to_start();
        assert_eq!(buf.text(), "");
        buf.undo();
        assert_eq!(buf.text(), "keep me");
        assert_eq!(buf.col, 7);
    }

    #[test]
    fn moving_by_display_lines_follows_the_wrap() {
        let mut buf = buffer("the quick brown fox jumps");
        let layout = text::layout(&buf.lines, 10);
        // "the quick" / "brown fox" / "jumps"
        buf.doc_start();
        assert_eq!((buf.row, buf.col), (0, 0));
        buf.move_display(&layout, 1);
        assert_eq!(buf.col, 10);
        buf.move_display(&layout, 1);
        assert_eq!(buf.col, 20);
        buf.move_display(&layout, -2);
        assert_eq!(buf.col, 0);
        buf.move_display(&layout, -5);
        assert_eq!(buf.col, 0, "clamped at the top");
    }

    #[test]
    fn counts_words() {
        let buf = buffer("one two  three\nfour");
        assert_eq!(buf.word_count(), 4);
        assert!(!buf.is_dirty());
        assert!(Buffer::new("   ").is_empty());
    }
}
