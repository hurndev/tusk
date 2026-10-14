//! Prose layout: wrapping text at word boundaries to a chosen width.
//!
//! Nothing here changes your file. Wrapping is a *view*: a source line is cut
//! into one or more [`Segment`]s that reference character ranges of the
//! original line, so the editor can map a cursor to a screen cell and the
//! reader can render pretty prose without touching a single byte.

use unicode_width::UnicodeWidthChar;

/// A horizontal slice of one source line, in character indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start: usize,
    pub end: usize,
}

/// One visible line on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayLine {
    /// Index into the original `lines` slice.
    pub row: usize,
    /// Character range within that line.
    pub start: usize,
    pub end: usize,
}

/// The wrapped view of a whole buffer.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub lines: Vec<DisplayLine>,
    /// For each source row, the inclusive range of display rows it occupies.
    pub per_row: Vec<(usize, usize)>,
}

/// How many terminal cells a character takes. Tabs count as four, matching
/// [`expand_tabs`], and control characters count as zero.
pub fn char_width(c: char) -> usize {
    match c {
        '\t' => 4,
        _ => c.width().unwrap_or(0),
    }
}

/// The width of text as it appears on screen.
///
/// Styling escapes are skipped rather than counted: measuring them as cells is
/// how a clipped line loses its closing reset.
pub fn visible_width(text: &str) -> usize {
    filter_visible(text, |_, _| true).1
}

/// Walk a string, copying SGR escape sequences through untouched and handing
/// the visible characters to `keep`, which receives the cells used so far and
/// this character's width. Returns what was kept, its width, and whether
/// anything was dropped.
fn filter_visible(text: &str, mut keep: impl FnMut(usize, usize) -> bool) -> (String, usize, bool) {
    let mut out = String::with_capacity(text.len());
    let mut used = 0usize;
    let mut truncated = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            out.push(c);
            if chars.peek() == Some(&'[') {
                out.push(chars.next().expect("peeked"));
                // ... up to and including the final byte of the sequence
                for n in chars.by_ref() {
                    out.push(n);
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else if let Some(n) = chars.next() {
                out.push(n);
            }
            continue;
        }
        if !keep(used, char_width(c)) {
            truncated = true;
            break;
        }
        used += char_width(c);
        out.push(c);
    }
    (out, used, truncated)
}

/// Clip text to a number of terminal cells, keeping escapes intact and never
/// ending on an unclosed style.
pub fn clip(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let (out, _, truncated) = filter_visible(text, |used, w| used + w <= max);
    if truncated && out.contains('\u{1b}') {
        format!("{out}\u{1b}[0m")
    } else {
        out
    }
}

fn chars_width(chars: &[char]) -> usize {
    chars.iter().copied().map(char_width).sum()
}

/// Render tabs as spaces for display only.
pub fn expand_tabs(s: &str) -> String {
    if s.contains('\t') {
        s.replace('\t', "    ")
    } else {
        s.to_string()
    }
}

/// Cut one source line into wrapped segments.
///
/// Breaks fall on spaces where possible; a single word wider than `width` is
/// split so nothing is ever lost. Spaces that sit exactly on a break are not
/// part of any segment, since they are invisible at the end of a line.
pub fn wrap_line(line: &str, width: usize) -> Vec<Segment> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let width = width.max(1);
    let mut segments: Vec<Segment> = Vec::new();

    let mut start = 0usize;
    let mut fitted = 0usize; // end index of the last *word* that fit
    let mut used = 0usize;
    let mut i = 0usize;

    while i < n {
        if chars[i] == ' ' || chars[i] == '\t' {
            let mut run = 0usize;
            while i < n && (chars[i] == ' ' || chars[i] == '\t') {
                run += char_width(chars[i]);
                i += 1;
            }
            if used + run <= width {
                used += run;
            } else {
                // A gap that cannot fit ends the line. Spaces that sit exactly
                // on the break belong to neither side, so the line reads as
                // prose rather than trailing whitespace.
                segments.push(Segment {
                    start,
                    end: if fitted > start { fitted } else { i },
                });
                start = i;
                used = 0;
                fitted = i;
            }
            continue;
        }

        let word_start = i;
        let mut word_width = 0usize;
        while i < n && chars[i] != ' ' && chars[i] != '\t' {
            word_width += char_width(chars[i]);
            i += 1;
        }
        let word_end = i;

        if used + word_width <= width {
            used += word_width;
            fitted = word_end;
            continue;
        }

        if used == 0 {
            // A word too long for an empty line: split it across lines.
            let mut cut = word_start;
            let mut taken = 0usize;
            while cut < word_end {
                let w = char_width(chars[cut]);
                if taken + w > width && taken > 0 {
                    break;
                }
                taken += w;
                cut += 1;
            }
            if cut == word_start {
                cut = word_start + 1; // always make progress
            }
            segments.push(Segment { start, end: cut });
            start = cut;
            i = cut;
            used = 0;
            fitted = cut;
            continue;
        }

        // Start a new line here and consider the same word again, so an
        // over-long word still gets split rather than overflowing.
        segments.push(Segment {
            start,
            end: if fitted > start { fitted } else { word_start },
        });
        start = word_start;
        used = 0;
        fitted = word_start;
        i = word_start;
    }

    if start < n || segments.is_empty() {
        segments.push(Segment { start, end: n });
    }
    segments
}

/// Wrap a whole buffer.
pub fn layout(lines: &[String], width: usize) -> Layout {
    let mut out = Layout {
        lines: Vec::new(),
        per_row: Vec::with_capacity(lines.len()),
    };
    for (row, line) in lines.iter().enumerate() {
        let first = out.lines.len();
        for segment in wrap_line(line, width) {
            out.lines.push(DisplayLine {
                row,
                start: segment.start,
                end: segment.end,
            });
        }
        let last = out.lines.len().saturating_sub(1).max(first);
        out.per_row.push((first, last));
    }
    out
}

/// Where a cursor sits on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorView {
    pub display_row: usize,
    /// Column in terminal cells from the start of the prose block.
    pub x: usize,
}

/// Translate a buffer position (row, char column) into a screen position.
pub fn locate(layout: &Layout, lines: &[String], row: usize, col: usize) -> CursorView {
    let Some(&(first, last)) = layout.per_row.get(row) else {
        return CursorView {
            display_row: 0,
            x: 0,
        };
    };
    let chars: Vec<char> = lines
        .get(row)
        .map(|l| l.chars().collect())
        .unwrap_or_default();

    let mut previous: Option<&DisplayLine> = None;
    for display_row in first..=last {
        let segment = &layout.lines[display_row];
        if col < segment.end {
            // A column inside a dropped space gap belongs to the line above.
            if col < segment.start
                && let Some(prev) = previous
            {
                return CursorView {
                    display_row: display_row - 1,
                    x: chars_width(&chars[prev.start..prev.end]),
                };
            }
            let clamped = col.clamp(segment.start, segment.end);
            return CursorView {
                display_row,
                x: chars_width(&chars[segment.start..clamped]),
            };
        }
        previous = Some(segment);
    }

    let segment = &layout.lines[last];
    CursorView {
        display_row: last,
        x: chars_width(&chars[segment.start..segment.end]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &str, segments: &[Segment]) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        segments
            .iter()
            .map(|s| chars[s.start..s.end].iter().collect())
            .collect()
    }

    #[test]
    fn wraps_on_word_boundaries() {
        let line = "the quick brown fox jumps over the lazy dog";
        let segments = wrap_line(line, 10);
        let pieces = text(line, &segments);
        assert_eq!(
            pieces,
            vec!["the quick", "brown fox", "jumps over", "the lazy", "dog"]
        );
        for piece in &pieces {
            assert!(visible_width(piece) <= 10, "{piece:?} is too wide");
        }
        // Nothing is lost but the spaces that landed on a break.
        assert_eq!(pieces.join(" "), line);
    }

    #[test]
    fn wraps_at_exactly_the_width() {
        let line = "abcd efgh";
        assert_eq!(text(line, &wrap_line(line, 9)), vec!["abcd efgh"]);
        assert_eq!(text(line, &wrap_line(line, 8)), vec!["abcd", "efgh"]);
    }

    #[test]
    fn splits_words_that_cannot_fit() {
        let line = "short aaaaaaaaaaaaaaaa";
        let pieces = text(line, &wrap_line(line, 8));
        assert_eq!(pieces, vec!["short", "aaaaaaaa", "aaaaaaaa"]);
        assert_eq!(pieces.concat(), "shortaaaaaaaaaaaaaaaa");

        let single = "abcdefghijklmnop";
        let pieces = text(single, &wrap_line(single, 6));
        assert_eq!(pieces, vec!["abcdef", "ghijkl", "mnop"]);
    }

    #[test]
    fn keeps_blank_and_indented_lines() {
        assert_eq!(text("", &wrap_line("", 10)), vec![""]);
        assert_eq!(text("   ", &wrap_line("   ", 10)), vec!["   "]);
        // Indentation survives wrapping of the first line.
        let line = "    indented prose that needs to wrap somewhere";
        let pieces = text(line, &wrap_line(line, 20));
        assert!(pieces[0].starts_with("    indented"));
        assert_eq!(pieces.join(" "), line);
    }

    #[test]
    fn handles_trailing_and_multiple_spaces() {
        let line = "a  b   c";
        assert_eq!(text(line, &wrap_line(line, 5)), vec!["a  b", "c"]);
        let line = "trailing space ";
        assert_eq!(text(line, &wrap_line(line, 40)), vec!["trailing space "]);
    }

    #[test]
    fn measures_wide_characters() {
        // Three two-cell glyphs are six cells wide.
        let line = "日本語";
        assert_eq!(visible_width(line), 6);
        assert_eq!(text(line, &wrap_line(line, 6)), vec!["日本語"]);
        assert_eq!(text(line, &wrap_line(line, 4)), vec!["日本", "語"]);
        assert_eq!(expand_tabs("a\tb"), "a    b");
        assert_eq!(char_width('\t'), 4);
    }

    #[test]
    fn measures_and_clips_styled_text_without_counting_escapes() {
        assert_eq!(visible_width("plain"), 5);
        assert_eq!(visible_width("\u{1b}[7mabc\u{1b}[0m"), 3);
        assert_eq!(visible_width("\u{1b}[1;30;43mhello\u{1b}[0m"), 5);
        assert_eq!(visible_width("日本"), 4);

        // Clipping a styled string must not eat the closing reset, or the
        // terminal stays wearing that style into the next frame.
        let styled = "\u{1b}[7mhello world\u{1b}[0m";
        let clipped = clip(styled, 5);
        assert_eq!(visible_width(&clipped), 5);
        assert!(
            clipped.ends_with("\u{1b}[0m"),
            "clipping must close the style: {clipped:?}"
        );
        assert!(clipped.starts_with("\u{1b}[7m"));

        // A styled string that already fits is returned untouched.
        let fits = "\u{1b}[36mok\u{1b}[0m";
        assert_eq!(clip(fits, 10), fits);

        // Escape sequences are neither split in half nor counted.
        let clipped = clip("\u{1b}[36m─────\u{1b}[0m", 2);
        assert_eq!(visible_width(&clipped), 2);
        let escapes: Vec<&str> = clipped
            .match_indices('\u{1b}')
            .map(|(i, _)| &clipped[i..])
            .collect();
        assert_eq!(
            escapes.len(),
            2,
            "expected the style and its close: {escapes:?}"
        );
        assert!(escapes[0].starts_with("\u{1b}[36m"), "colour kept whole");
        assert!(escapes[1].starts_with("\u{1b}[0m"), "style closed");
    }

    #[test]
    fn lays_out_a_buffer_with_row_ranges() {
        let lines: Vec<String> = vec!["one two three four".into(), String::new(), "five".into()];
        let layout = layout(&lines, 9);
        // "one two" / "three" / "four" / "" / "five"
        assert_eq!(layout.lines.len(), 5);
        assert_eq!(layout.per_row[0], (0, 2));
        assert_eq!(layout.per_row[1], (3, 3));
        assert_eq!(layout.per_row[2], (4, 4));
        for display in &layout.lines {
            let chars: Vec<char> = lines[display.row].chars().collect();
            assert!(chars_width(&chars[display.start..display.end]) <= 9);
        }
    }

    #[test]
    fn locates_the_cursor_on_screen() {
        let lines: Vec<String> = vec!["the quick brown".into()];
        let layout = layout(&lines, 10);
        // Segments are "the quick" (0..9) and "brown" (10..15); index 9 is the
        // space that landed on the break.
        assert_eq!(locate(&layout, &lines, 0, 0).display_row, 0);
        assert_eq!(locate(&layout, &lines, 0, 0).x, 0);
        assert_eq!(
            locate(&layout, &lines, 0, 8),
            CursorView {
                display_row: 0,
                x: 8
            }
        );
        // A column on the dropped space sits at the end of the first line.
        assert_eq!(
            locate(&layout, &lines, 0, 9),
            CursorView {
                display_row: 0,
                x: 9
            }
        );
        // The first character of the next word starts the second line.
        assert_eq!(
            locate(&layout, &lines, 0, 10),
            CursorView {
                display_row: 1,
                x: 0
            }
        );
        assert_eq!(
            locate(&layout, &lines, 0, 11),
            CursorView {
                display_row: 1,
                x: 1
            }
        );
        // The end of the line is the end of the last display row.
        assert_eq!(
            locate(&layout, &lines, 0, 15),
            CursorView {
                display_row: 1,
                x: 5
            }
        );
    }
}
