//! The journal file: parsing entries, appending, editing, deleting, backing up.
//!
//! Entries are stored as `[YYYY-MM-DD HH:MM:SS AM] body`, separated by blank
//! lines. That is the format used by jrnl, so an existing journal opens
//! unchanged and any other tool can still read the file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Local, NaiveDate, NaiveDateTime};
use regex::Regex;

/// The timestamp format tusk writes: `2026-10-14 01:43:09 PM` (jrnl's `%F %r`).
pub const STAMP: &str = "%Y-%m-%d %I:%M:%S %p";

/// A short format used for listings.
pub const STAMP_SHORT: &str = "%Y-%m-%d %H:%M";

#[derive(Debug, Clone)]
pub struct Entry {
    pub date: NaiveDateTime,
    pub body: String,
    /// Byte offset of the `[` that opens the header.
    pub start: usize,
    /// Byte offset just past the last byte of the body.
    pub end: usize,
}

impl Entry {
    pub fn first_line_ref(&self) -> &str {
        self.body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
    }

    pub fn first_line(&self) -> String {
        self.first_line_ref().to_string()
    }

    pub fn words(&self) -> usize {
        self.body.split_whitespace().count()
    }

    pub fn lines(&self) -> usize {
        self.body.lines().count()
    }

    pub fn tags(&self) -> Vec<String> {
        let re = tag_regex();
        let mut out = Vec::new();
        for m in re.find_iter(&self.body) {
            let boundary_ok = match self.body[..m.start()].chars().next_back() {
                None => true,
                Some(c) => !c.is_alphanumeric() && !matches!(c, '#' | '@' | '_' | '-' | '/'),
            };
            if boundary_ok {
                out.push(m.as_str().to_string());
            }
        }
        out
    }
}

pub fn tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[#@][\p{L}\p{N}_][\p{L}\p{N}_\-/]*").expect("valid regex"))
}

fn header_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^[ \t]*\[(\d{4}-\d{2}-\d{2}[ T]\d{1,2}:\d{2}(?::\d{2})?(?:\s*[AaPp]\.?[Mm]\.?)?)\]",
        )
        .expect("valid regex")
    })
}

/// Parse every entry in a journal file, remembering byte offsets so edits can
/// be surgical rather than rewriting the whole file.
pub fn parse(content: &str) -> Vec<Entry> {
    let re = header_regex();
    let mut out: Vec<Entry> = Vec::new();
    // date, header start, body start, body end
    let mut current: Option<(NaiveDateTime, usize, usize, usize)> = None;
    let mut offset = 0usize;

    for raw in content.split_inclusive('\n') {
        let line_start = offset;
        offset += raw.len();
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);

        if let Some(caps) = re.captures(line) {
            if let Some(entry) = current.take().map(|c| finish(content, c)) {
                out.push(entry);
            }
            let stamp = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            if let Some(date) = parse_stamp(stamp) {
                let body_start = line_start + caps.get(0).map(|m| m.end()).unwrap_or(line.len());
                let mut body_start = body_start;
                // skip a single separating space after the closing bracket
                if content[body_start..].starts_with(' ') {
                    body_start += 1;
                }
                current = Some((date, line_start, body_start, line_start + line.len()));
                continue;
            }
            // A bracket line with an unparseable date is ordinary prose.
        }

        if let Some((date, start, body_start, body_end)) = current {
            let new_end = if line.trim().is_empty() {
                body_end
            } else {
                line_start + line.len()
            };
            current = Some((date, start, body_start, new_end));
        }
    }

    if let Some(entry) = current.map(|c| finish(content, c)) {
        out.push(entry);
    }
    out
}

fn finish(
    content: &str,
    (date, start, body_start, body_end): (NaiveDateTime, usize, usize, usize),
) -> Entry {
    let body_start = body_start.min(content.len());
    let body_end = body_end.min(content.len()).max(body_start);
    let body = content[body_start..body_end].trim_end().to_string();
    Entry {
        date,
        body,
        start,
        end: body_end,
    }
}

/// Parse the timestamp inside an entry header.
pub fn parse_stamp(raw: &str) -> Option<NaiveDateTime> {
    let cleaned = raw.trim().replace('.', "");
    let cleaned = cleaned.replace('T', " ");
    let normalized = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // chrono wants an unambiguous meridiem marker.
    let normalized = match normalized.rsplit_once(' ') {
        Some((head, tail))
            if tail.eq_ignore_ascii_case("am") || tail.eq_ignore_ascii_case("pm") =>
        {
            format!("{head} {}", tail.to_ascii_uppercase())
        }
        _ => normalized,
    };

    const FORMATS: [&str; 6] = [
        "%Y-%m-%d %I:%M:%S %p",
        "%Y-%m-%d %I:%M %p",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %I %p",
        "%Y-%m-%d",
    ];
    FORMATS
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(&normalized, fmt).ok())
}

/// Parse a user-supplied timestamp, e.g. `2026-03-14 21:30`.
pub fn parse_datetime_spec(spec: &str) -> Result<NaiveDateTime> {
    if let Some(dt) = parse_stamp(spec) {
        return Ok(dt);
    }
    if let Ok(date) = NaiveDate::parse_from_str(spec.trim(), "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).expect("valid midnight"));
    }
    bail!("`{spec}` is not a date and time (try \"2026-03-14 21:30\")")
}

pub struct Journal {
    pub path: PathBuf,
    pub content: String,
    pub entries: Vec<Entry>,
}

impl Journal {
    /// Load a journal. A missing file is an empty journal, not an error.
    pub fn load(path: &Path) -> Result<Self> {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => {
                return Err(err).with_context(|| format!("cannot read {}", path.display()));
            }
        };
        let entries = parse(&content);
        Ok(Journal {
            path: path.to_path_buf(),
            content,
            entries,
        })
    }

    pub fn reload(&mut self) {
        self.entries = parse(&self.content);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append an entry and refresh the in-memory parse.
    pub fn append(&mut self, date: NaiveDateTime, body: &str) {
        let block = format!("[{}] {}", date.format(STAMP), body.trim_end());
        let base = self.content.trim_end();
        self.content = if base.is_empty() {
            format!("{block}\n")
        } else {
            format!("{base}\n\n{block}\n")
        };
        self.reload();
    }

    /// Replace entry `index` (file order) with new text.
    pub fn replace(&mut self, index: usize, date: NaiveDateTime, body: &str) {
        let (start, end) = {
            let entry = &self.entries[index];
            (entry.start, entry.end)
        };
        let block = format!("[{}] {}", date.format(STAMP), body.trim_end());
        self.content = format!(
            "{}{}{}",
            &self.content[..start],
            block,
            &self.content[end..]
        );
        self.reload();
    }

    /// Remove entry `index` (file order), tidying the seam it leaves behind.
    pub fn remove(&mut self, index: usize) {
        let (start, end) = {
            let entry = &self.entries[index];
            (entry.start, entry.end)
        };
        let left = self.content[..start].trim_end();
        let right = self.content[end..].trim_start_matches(['\n', '\r']);
        let mut out = String::from(left);
        if !right.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(right);
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        self.content = out;
        self.reload();
    }

    /// Write the journal back, atomically, optionally keeping a backup.
    pub fn save(&self, backup: bool) -> Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        if backup
            && self.path.exists()
            && let Err(err) = backup_file(&self.path)
        {
            crate::render::note(&format!("tusk: warning: could not back up: {err:#}"));
        }
        let tmp = temp_sibling(&self.path);
        fs::write(&tmp, &self.content)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("cannot replace {}", self.path.display()))?;
        Ok(())
    }

    pub fn total_words(&self) -> usize {
        self.entries.iter().map(Entry::words).sum()
    }

    /// Tag counts across all entries, most used first.
    pub fn tag_counts(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for entry in &self.entries {
            for tag in entry.tags() {
                let key = tag.to_lowercase();
                let slot = counts.entry(key).or_insert_with(|| (tag.clone(), 0));
                slot.1 += 1;
            }
        }
        let mut out: Vec<(String, usize)> = counts.into_values().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

/// How many backups to keep per journal.
const BACKUPS_KEPT: usize = 10;

fn backup_file(path: &Path) -> Result<PathBuf> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tusk-backups");
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "journal".to_string());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "txt".to_string());
    let stamp = Local::now().format("%Y%m%d-%H%M%S%.3f");
    let target = dir.join(format!("{stem}-{stamp}.{ext}"));
    fs::copy(path, &target)
        .with_context(|| format!("cannot copy {} to {}", path.display(), target.display()))?;

    prune_backups(&dir, &stem, &ext);
    Ok(target)
}

fn prune_backups(dir: &Path, stem: &str, ext: &str) {
    let prefix = format!("{stem}-");
    let suffix = format!(".{ext}");
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut names: Vec<String> = read
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(&suffix))
        .collect();
    // Timestamped names sort chronologically.
    names.sort();
    let excess = names.len().saturating_sub(BACKUPS_KEPT);
    for name in names.into_iter().take(excess) {
        let _ = fs::remove_file(dir.join(name));
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "journal.txt".to_string());
    let tmp_name = format!(".{name}.tusk-{}", std::process::id());
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent.join(tmp_name),
        None => PathBuf::from(tmp_name),
    }
}

/// Resolve an entry reference to indices in file order (oldest first).
///
/// Accepts `1` (newest-first numbering), `latest`, `oldest`, a date prefix
/// such as `2026-03-14` or `2026-03`, or a full timestamp prefix.
pub fn resolve(entries: &[Entry], spec: &str) -> Result<Vec<usize>> {
    let total = entries.len();
    if total == 0 {
        bail!("the journal is empty — write something with `tusk new`");
    }
    let spec = spec.trim();
    match spec.to_ascii_lowercase().as_str() {
        "latest" | "last" | "newest" => return Ok(vec![total - 1]),
        "oldest" | "first" => return Ok(vec![0]),
        _ => {}
    }

    if let Ok(n) = spec.parse::<usize>() {
        if n == 0 {
            bail!("entry numbers start at 1 (1 = the newest entry)");
        }
        if n > total {
            bail!("there is no entry {n}; the journal has {total} entries (1 = newest)");
        }
        return Ok(vec![total - n]);
    }

    // A full timestamp prefix, e.g. "2026-10-14 13:43".
    let stamp = spec
        .replace('T', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let stamp_hits: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.date
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
                .starts_with(&stamp)
        })
        .map(|(i, _)| i)
        .collect();
    if !stamp_hits.is_empty() {
        return Ok(stamp_hits);
    }

    let (from, to) = crate::query::parse_range(spec)
        .map_err(|_| anyhow!("`{spec}` is not an entry number, keyword, or date"))?;
    let hits: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let day = e.date.date();
            day >= from && day <= to
        })
        .map(|(i, _)| i)
        .collect();
    if hits.is_empty() {
        bail!("no entries between {from} and {to}");
    }
    Ok(hits)
}

/// Newest-first entry number for a file-order index.
pub fn entry_number(total: usize, index: usize) -> usize {
    total - index
}

/// Warn when prose would be mistaken for a new entry header later on.
pub fn looks_like_header(body: &str) -> bool {
    header_regex().is_match(body.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[2024-08-27 12:00:00 AM] The kettle broke this morning.\nIt took twenty minutes to sort out.\n\n[2024-08-28 10:30:00 PM] A quiet day.\n\nSecond paragraph.\n";

    #[test]
    fn parses_entries_with_offsets() {
        let entries = parse(SAMPLE);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].first_line(), "The kettle broke this morning.");
        assert_eq!(
            entries[0].body,
            "The kettle broke this morning.\nIt took twenty minutes to sort out."
        );
        assert_eq!(entries[1].body, "A quiet day.\n\nSecond paragraph.");
        assert_eq!(entries[1].date.to_string(), "2024-08-28 22:30:00");

        // Offsets must slice the original file exactly.
        let first = &SAMPLE[entries[0].start..entries[0].end];
        assert_eq!(
            first,
            "[2024-08-27 12:00:00 AM] The kettle broke this morning.\nIt took twenty minutes to sort out."
        );
        let second = &SAMPLE[entries[1].start..entries[1].end];
        assert_eq!(
            second,
            "[2024-08-28 10:30:00 PM] A quiet day.\n\nSecond paragraph."
        );
    }

    #[test]
    fn handles_crlf_empty_body_and_blank_gaps() {
        let text = "[2024-01-01 09:00:00 AM] One.\r\n\r\n\r\n[2024-01-02 09:00:00 AM] Two.\r\n";
        let entries = parse(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].body, "One.");
        assert_eq!(entries[1].body, "Two.");

        let empty = parse("[2024-01-01 09:00:00 AM]\n\n[2024-01-02 09:00:00 AM] Hi.\n");
        assert_eq!(empty.len(), 2);
        assert_eq!(empty[0].body, "");
    }

    #[test]
    fn ignores_bracket_lines_that_are_not_dates() {
        let entries = parse("[not a date] hello\n\n[2024-01-01 09:00:00 AM] Real.\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, "Real.");
    }

    #[test]
    fn tags_are_extracted_with_boundaries() {
        let mut entry = parse("[2024-01-01 09:00:00 AM] hi\n").remove(0);
        entry.body =
            "Spoke to @alice about the #garden, and #self-care. Email me@example.com or a#b."
                .into();
        let tags = entry.tags();
        assert!(tags.contains(&"#garden".to_string()));
        assert!(tags.contains(&"@alice".to_string()));
        assert!(tags.contains(&"#self-care".to_string()));
        assert!(!tags.iter().any(|t| t == "#example" || t == "#b"));
    }

    #[test]
    fn append_roundtrips_through_parse() {
        let mut journal = Journal {
            path: PathBuf::from("/tmp/does-not-matter.txt"),
            content: SAMPLE.to_string(),
            entries: parse(SAMPLE),
        };
        let when = parse_stamp("2026-10-14 01:43:09 PM").unwrap();
        journal.append(when, "A new thought.\nSecond line.");
        assert_eq!(journal.entries.len(), 3);
        assert_eq!(journal.entries[2].body, "A new thought.\nSecond line.");
        assert_eq!(journal.entries[2].date.to_string(), "2026-10-14 13:43:09");
        assert!(journal.content.ends_with("Second line.\n"));
        // The parse of the rewritten text must match the in-memory entries.
        assert_eq!(parse(&journal.content).len(), 3);
    }

    #[test]
    fn replace_and_remove_keep_the_file_clean() {
        let mut journal = Journal {
            path: PathBuf::from("/tmp/does-not-matter.txt"),
            content: SAMPLE.to_string(),
            entries: parse(SAMPLE),
        };
        let when = parse_stamp("2024-08-27 12:00:00 AM").unwrap();
        journal.replace(0, when, "Edited first.");
        assert!(
            journal
                .content
                .starts_with("[2024-08-27 12:00:00 AM] Edited first.\n\n[2024-08-28")
        );
        assert_eq!(journal.entries.len(), 2);

        journal.remove(0);
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].body, "A quiet day.\n\nSecond paragraph.");
        assert!(
            journal
                .content
                .starts_with("[2024-08-28 10:30:00 PM] A quiet day.")
        );
        assert!(!journal.content.contains("\n\n\n"));
    }

    #[test]
    fn numbers_entries_from_the_newest() {
        let entries = parse(SAMPLE);
        assert_eq!(resolve(&entries, "1").unwrap(), vec![1]);
        assert_eq!(resolve(&entries, "2").unwrap(), vec![0]);
        assert_eq!(resolve(&entries, "latest").unwrap(), vec![1]);
        assert_eq!(resolve(&entries, "oldest").unwrap(), vec![0]);
        assert_eq!(resolve(&entries, "2024-08-27").unwrap(), vec![0]);
        assert_eq!(resolve(&entries, "2024-08").unwrap(), vec![0, 1]);
        assert_eq!(resolve(&entries, "2024-08-28 10:30").unwrap(), vec![1]);
        assert!(resolve(&entries, "3").is_err());
        assert!(resolve(&entries, "0").is_err());
        assert!(resolve(&entries, "1999").is_err());
    }

    #[test]
    fn parses_jrnl_meridiem_variants() {
        assert!(parse_stamp("2026-10-14 01:43:09 PM").is_some());
        assert!(parse_stamp("2026-10-14 1:43 pm").is_some());
        assert!(parse_stamp("2026-10-14 13:43").is_some());
        assert!(parse_stamp("2026-10-14T13:43:09").is_some());
        assert!(parse_stamp("nonsense").is_none());
    }
}
