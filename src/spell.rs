//! Spellchecking.
//!
//! Deliberately small and lazy. There is no affix engine and no process to
//! talk to: a word list is read once, and only the text on screen is ever
//! checked, with results cached per word so a screenful costs a handful of
//! hash lookups per keystroke.
//!
//! Two things keep the false-positive rate low with a plain word list: inflected
//! forms are accepted when their stem is known (`walked`, `running`, `carries`),
//! and anything that is not plainly a word is skipped (tags, acronyms, tokens
//! joined to digits).

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

use crate::config::Config;

/// Endings accepted on top of a dictionary stem.
const SUFFIXES: [&str; 7] = ["s", "es", "ed", "ing", "ly", "er", "est"];

/// Clitics accepted on top of a known stem: `Sam's`, `you'd`, `we'll`.
const CLITICS: [&str; 6] = ["'s", "'d", "'ll", "'ve", "'re", "'m"];

/// Contractions whose stem is not a word on its own.
const CONTRACTIONS: [&str; 40] = [
    "aren't",
    "can't",
    "couldn't",
    "didn't",
    "doesn't",
    "don't",
    "hadn't",
    "hasn't",
    "haven't",
    "he'd",
    "he'll",
    "he's",
    "here's",
    "how's",
    "i'd",
    "i'll",
    "i'm",
    "i've",
    "isn't",
    "it'd",
    "it'll",
    "it's",
    "let's",
    "mustn't",
    "o'clock",
    "shan't",
    "she'd",
    "she'll",
    "she's",
    "shouldn't",
    "that's",
    "there's",
    "they'd",
    "they'll",
    "they're",
    "they've",
    "wasn't",
    "we'd",
    "we're",
    "weren't",
];

/// A checkable word, as character indices (for styling) and a byte range (for
/// slicing, so checking a word allocates nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word {
    pub start: usize,
    pub end: usize,
    pub bytes: (usize, usize),
}

/// The words in a line that are worth checking.
///
/// Skips `#tags` and `@names`, all-caps acronyms, single letters that are not
/// `a`/`I`, and anything stuck to a digit (`10th`, `1980s`).
pub fn words(line: &str) -> Vec<Word> {
    let chars: Vec<char> = line.chars().collect();
    // Byte offset of each character, plus the end, so slicing a word is O(1).
    let mut byte_of: Vec<usize> = line.char_indices().map(|(b, _)| b).collect();
    byte_of.push(line.len());

    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !is_word_char(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        let end = i;

        let lead = start
            + chars[start..end]
                .iter()
                .take_while(|c| matches!(c, '\'' | '-'))
                .count();
        let close = end
            - chars[start..end]
                .iter()
                .rev()
                .take_while(|c| matches!(c, '\'' | '-'))
                .count();
        if close <= lead {
            continue;
        }
        let token: String = chars[lead..close].iter().collect();

        // A word glued to a tag symbol or a digit is not prose.
        let before = start.checked_sub(1).and_then(|p| chars.get(p)).copied();
        if before.is_some_and(|c| matches!(c, '#' | '@') || c.is_ascii_digit()) {
            continue;
        }
        // Acronyms are not ours to judge.
        if token.chars().count() > 1
            && token.chars().all(|c| !c.is_lowercase())
            && token.chars().any(|c| c.is_uppercase())
        {
            continue;
        }
        if token.chars().count() == 1 && !token.eq_ignore_ascii_case("a") && token != "I" {
            continue;
        }
        out.push(Word {
            start: lead,
            end: close,
            bytes: (byte_of[lead], byte_of[close]),
        });
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c == '\'' || c == '-'
}

/// The word under a character column, if there is one.
pub fn word_at(line: &str, col: usize) -> Option<Word> {
    words(line)
        .into_iter()
        .find(|w| col >= w.start && col <= w.end)
}

/// Why spellchecking could not start, in words a person can act on.
pub enum Status {
    Ready(Box<Dictionary>),
    Disabled(String),
}

pub struct Dictionary {
    known: HashSet<Box<str>>,
    /// Verdict per word, so a redraw re-decides nothing.
    cache: HashMap<Box<str>, bool>,
    /// e.g. "en_GB", or the file stem when a path was given.
    pub language: String,
    pub sources: Vec<PathBuf>,
    pub base_words: usize,
    /// Where personal words are appended, if there is anywhere to put them.
    pub personal: Option<PathBuf>,
    pub personal_words: usize,
}

impl Dictionary {
    /// Load the configured dictionary plus any personal word list.
    pub fn load(cfg: &Config) -> Status {
        if !cfg.spellcheck {
            return Status::Disabled("spellcheck is off".to_string());
        }

        // $TUSK_DICTIONARY overrides the config, as tusk's other settings allow.
        let requested = env::var("TUSK_DICTIONARY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| cfg.dictionary.clone());

        let mut sources = Vec::new();
        let mut words: HashSet<Box<str>> = HashSet::new();
        let mut language = requested.clone().unwrap_or_else(|| "en_GB".to_string());

        let base = match &requested {
            Some(value) if looks_like_path(value) => {
                let path = crate::config::expand(Path::new(value));
                if path.is_file() {
                    language = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or(language);
                    Some(path)
                } else {
                    return Status::Disabled(format!(
                        "dictionary file not found: {}",
                        path.display()
                    ));
                }
            }
            Some(tag) => find_tagged(tag),
            None => find_tagged("en_GB"),
        };

        let Some(base) = base else {
            let tag = requested.unwrap_or_else(|| "en_GB".to_string());
            return Status::Disabled(format!(
                "no {tag} dictionary found — install a word list (Arch: \
                 sudo pacman -S words, Debian: sudo apt install wbritish), or point \
                 `dictionary` at one"
            ));
        };

        let is_hunspell = base
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dic"));
        match fs::read_to_string(&base) {
            Ok(text) => {
                load_words(&text, is_hunspell, &mut words);
                sources.push(base);
            }
            Err(err) => {
                return Status::Disabled(format!("cannot read {}: {err}", base.display()));
            }
        }
        let base_words = words.len();

        // Personal words always join in, and may not exist yet: a word added
        // with F3 creates the file.
        let personal = cfg.personal_words.clone().or_else(|| {
            crate::config::config_dir()
                .ok()
                .map(|d| d.join("words.txt"))
        });
        let mut personal_words = 0usize;
        if let Some(path) = &personal
            && let Ok(text) = fs::read_to_string(path)
        {
            let before = words.len();
            load_words(&text, false, &mut words);
            personal_words = text
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .count();
            if words.len() > before {
                sources.push(path.clone());
            }
        }

        Status::Ready(Box::new(Dictionary {
            known: words,
            cache: HashMap::new(),
            language,
            sources,
            base_words,
            personal,
            personal_words,
        }))
    }

    /// Is this word unknown, ignoring case and accepting inflections?
    pub fn is_unknown(&mut self, word: &str) -> bool {
        let lower = word.to_lowercase();
        match self.cache.get(lower.as_str()) {
            Some(&known) => !known,
            None => {
                let known = self.known_inner(&lower);
                self.cache.insert(lower.into_boxed_str(), known);
                !known
            }
        }
    }

    /// Character spans of the unknown words in a line.
    pub fn unknown_spans(&mut self, line: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for word in words(line) {
            let (b0, b1) = word.bytes;
            if b1 <= b0 || b1 > line.len() {
                continue;
            }
            if self.is_unknown(&line[b0..b1]) {
                out.push((word.start, word.end));
            }
        }
        out
    }

    fn known_inner(&self, lower: &str) -> bool {
        if self.known.contains(lower) {
            return true;
        }
        // self-care is fine when self and care are.
        if lower.contains('-') {
            let parts: Vec<&str> = lower.split('-').filter(|p| !p.is_empty()).collect();
            return !parts.is_empty() && parts.iter().all(|part| self.known_inner(part));
        }
        if let Some(expansion) = contraction_stem(lower) {
            return expansion;
        }
        if lower.contains('\'') {
            for clitic in CLITICS {
                if let Some(stem) = lower.strip_suffix(clitic)
                    && stem.len() >= 2
                    && self.known.contains(stem)
                {
                    return true;
                }
            }
            return false;
        }
        // carries -> carry, carried -> carry
        for (ending, replacement) in [("ies", "y"), ("ied", "y")] {
            if let Some(stem) = lower.strip_suffix(ending) {
                let candidate = format!("{stem}{replacement}");
                if self.known.contains(candidate.as_str()) {
                    return true;
                }
            }
        }
        for suffix in SUFFIXES {
            let Some(stem) = lower.strip_suffix(suffix) else {
                continue;
            };
            if stem.chars().count() < 3 {
                continue;
            }
            if self.known.contains(stem) {
                return true;
            }
            // making -> make
            let with_e = format!("{stem}e");
            if self.known.contains(with_e.as_str()) {
                return true;
            }
            // running -> run, stopped -> stop
            if let Some(single) = undouble(stem)
                && self.known.contains(single.as_str())
            {
                return true;
            }
        }
        false
    }

    /// Remember a word the reader vouches for, and keep it for next time.
    /// Returns false when it was already there.
    pub fn add_word(&mut self, word: &str) -> Result<bool> {
        let trimmed = word.trim_matches(|c: char| !c.is_alphabetic() && c != '\'' && c != '-');
        if trimmed.is_empty() {
            anyhow::bail!("no word to add");
        }
        let lower = trimmed.to_lowercase();
        self.known.insert(lower.clone().into_boxed_str());
        self.cache.insert(lower.clone().into_boxed_str(), true);

        let Some(path) = &self.personal else {
            anyhow::bail!("no personal word list to write to");
        };
        // Pressing F3 twice should not grow the file.
        if let Ok(existing) = fs::read_to_string(path)
            && existing
                .lines()
                .any(|l| l.trim().eq_ignore_ascii_case(trimmed))
        {
            return Ok(false);
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let fresh = !path.exists();
        let mut text = if fresh {
            "# Words tusk should accept. One per line.\n".to_string()
        } else {
            let existing = fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            if existing.is_empty() || existing.ends_with('\n') {
                existing
            } else {
                format!("{existing}\n")
            }
        };
        text.push_str(trimmed);
        text.push('\n');
        fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
        self.personal_words += 1;
        Ok(true)
    }

    /// Where the dictionary came from, shortened for a config listing.
    pub fn source_note(&self) -> Option<String> {
        self.sources.first().map(|p| shorten(p))
    }

    /// e.g. "en_GB · 54,763 words · ~/.config/tusk/words.txt"
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} · {} words", self.language, self.base_words)];
        if let Some(path) = &self.personal
            && self.personal_words > 0
        {
            parts.push(format!(
                "{} personal ({})",
                self.personal_words,
                shorten(path)
            ));
        }
        parts.join(" · ")
    }
}

fn contraction_stem(lower: &str) -> Option<bool> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let set = SET.get_or_init(|| CONTRACTIONS.into_iter().collect());
    if set.contains(lower) {
        return Some(true);
    }
    // isn't / wasn't / didn't: the stem is a known word in every case but
    // "isn", so strip "n't" and try the stem, then the stem plus "s".
    if let Some(stem) = lower.strip_suffix("n't") {
        return Some(!stem.is_empty() && (stem.ends_with('i') || stem.ends_with('o')));
    }
    None
}

/// `runn` -> `run`, `stopp` -> `stop`. Only doubled consonants, so `seeing`
/// never collapses to `se`.
fn undouble(stem: &str) -> Option<String> {
    let mut chars = stem.chars().rev();
    let last = chars.next()?;
    if !last.is_ascii_alphabetic() || "aeiou".contains(last) {
        return None;
    }
    if chars.next() == Some(last) {
        let mut out = stem.to_string();
        out.pop();
        return Some(out);
    }
    None
}

/// Read a word list: either a hunspell `.dic` (flags after `/` are dropped, and
/// entries marked with `-`/`+` are skipped) or one word per line.
fn load_words(text: &str, hunspell: bool, into: &mut HashSet<Box<str>>) {
    // A few lists begin with a byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if hunspell {
        lines.next(); // the leading count
    }
    for line in lines {
        let entry = if hunspell {
            line.split(['/', '\t']).next().unwrap_or("").trim()
        } else {
            line.trim()
        };
        if entry.is_empty()
            || entry.starts_with('#')
            || entry.starts_with('-')
            || entry.starts_with('+')
        {
            continue;
        }
        let entry = entry.split_whitespace().next().unwrap_or(entry);
        if entry.is_empty() {
            continue;
        }
        into.insert(entry.to_lowercase().into_boxed_str());
    }
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/')
        || value.contains('\\')
        || value.starts_with('~')
        || value.ends_with(".dic")
        || value.ends_with(".txt")
        || value.ends_with(".words")
}

/// Conventional places a language tag might live.
fn find_tagged(tag: &str) -> Option<PathBuf> {
    let mut names: Vec<String> = vec![tag.to_string()];
    let underscored = tag.replace('-', "_");
    let dashed = tag.replace('_', "-");
    if !names.contains(&underscored) {
        names.push(underscored.clone());
    }
    if !names.contains(&dashed) {
        names.push(dashed.clone());
    }
    names.push(dashed.to_lowercase());
    names.push(underscored.to_lowercase());
    // hunspell ships a "large" variant; some distros use it.
    for name in names.clone() {
        names.push(format!("{name}-large"));
    }

    // Plain word lists are named after the dialect instead.
    let mut lists: Vec<&str> = Vec::new();
    match tag.replace('-', "_").to_lowercase().as_str() {
        "en_gb" | "en_uk" => lists.extend(["british-english", "british", "english", "words"]),
        "en_us" | "en" => lists.extend(["american-english", "english", "words"]),
        _ => lists.extend(["english", "words"]),
    }

    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/usr/share/hunspell"),
        PathBuf::from("/usr/share/myspell/dicts"),
        PathBuf::from("/usr/share/myspell"),
        PathBuf::from("/usr/local/share/hunspell"),
        PathBuf::from("/usr/share/dict"),
        PathBuf::from("/usr/local/share/dict"),
        // macOS keeps hunspell dictionaries here.
        PathBuf::from("/Library/Spelling"),
    ];
    if let Ok(home) = crate::config::home_dir() {
        dirs.push(home.join(".local/share/hunspell"));
        dirs.push(home.join(".hunspell"));
        dirs.push(home.join("Library/Spelling"));
    }
    if let Ok(dir) = crate::config::config_dir() {
        dirs.push(dir);
    }

    // Plain, fully expanded word lists come first: they need no affix rules,
    // so nothing a real dictionary knows is missing from them. `british-english`
    // from the `words` package is the best dictionary to point tusk at.
    for dir in &dirs {
        for name in &lists {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Then hunspell dictionaries. Their `.aff` rules are not read, so a few
    // words built purely from those rules (`every`, `many`, `has`) can be
    // underlined; the suffix rules above cover most such cases.
    for dir in &dirs {
        for name in &names {
            let candidate = dir.join(format!("{name}.dic"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn shorten(path: &Path) -> String {
    if let Ok(home) = crate::config::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = path.display().to_string().strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict_from(words: &[&str]) -> Dictionary {
        Dictionary {
            known: words.iter().map(|w| (*w).into()).collect(),
            cache: HashMap::new(),
            language: "test".into(),
            sources: Vec::new(),
            base_words: words.len(),
            personal: None,
            personal_words: 0,
        }
    }

    #[test]
    fn finds_words_and_skips_what_is_not_prose() {
        let line = "The #garden needs @alice's 10th 1980s BBC help, a I.";
        let found: Vec<&str> = words(line)
            .iter()
            .map(|w| &line[w.bytes.0..w.bytes.1])
            .collect();
        assert_eq!(found, vec!["The", "needs", "help", "a", "I"]);
    }

    #[test]
    fn word_spans_are_character_indices() {
        // Multibyte before the word must not skew the spans.
        let line = "héllo wörld";
        let spans = words(line);
        assert_eq!(spans.len(), 2);
        let chars: Vec<char> = line.chars().collect();
        let first: String = chars[spans[0].start..spans[0].end].iter().collect();
        let second: String = chars[spans[1].start..spans[1].end].iter().collect();
        assert_eq!(first, "héllo");
        assert_eq!(second, "wörld");
    }

    #[test]
    fn finds_the_word_under_a_column() {
        let line = "the kettle broke";
        let word = word_at(line, 6).expect("a word");
        assert_eq!(&line[word.bytes.0..word.bytes.1], "kettle");
        // A cursor sitting just after a word is still on it, so F3 works
        // straight after typing one.
        let after = word_at(line, 3).expect("end of the first word");
        assert_eq!(&line[after.bytes.0..after.bytes.1], "the");
        // In the middle of a run of spaces there is no word.
        assert_eq!(word_at("the  kettle", 4), None);
    }

    #[test]
    fn accepts_inflections_of_known_stems() {
        let mut dict = dict_from(&[
            "walk", "run", "carry", "make", "stop", "day", "quick", "glass",
        ]);
        for word in [
            "walks", "walked", "walking", "running", "runs", "carries", "carried", "making",
            "stops", "stopped", "days", "quicker", "quickest", "quickly", "glass",
        ] {
            assert!(!dict.is_unknown(word), "{word} should be accepted");
        }
        // ...but not everything that merely ends in a suffix.
        for word in ["walkedd", "runnning", "makking", "zzzing"] {
            assert!(dict.is_unknown(word), "{word} should not be accepted");
        }
    }

    #[test]
    fn accepts_contractions_and_clitics() {
        let mut dict = dict_from(&["sam", "you", "we", "it", "do", "is"]);
        for word in [
            "Sam's", "you'd", "we'll", "it's", "don't", "isn't", "can't", "o'clock",
        ] {
            assert!(!dict.is_unknown(word), "{word} should be accepted");
        }
        assert!(dict.is_unknown("sam'z"));
    }

    #[test]
    fn accepts_hyphenated_words_when_every_part_is_known() {
        let mut dict = dict_from(&["self", "care", "well", "known"]);
        assert!(!dict.is_unknown("self-care"));
        assert!(!dict.is_unknown("well-known"));
        assert!(dict.is_unknown("self-carr"));
    }

    #[test]
    fn case_does_not_matter() {
        let mut dict = dict_from(&["london"]);
        assert!(!dict.is_unknown("London"));
        assert!(!dict.is_unknown("LONDON"));
        assert!(dict.is_unknown("londom"));
    }

    #[test]
    fn reads_hunspell_dictionaries() {
        let dic = "3\nwalk/AB\nforbidden-\nColour/MS\ndon't\n";
        let mut set = HashSet::new();
        load_words(dic, true, &mut set);
        assert!(set.contains("walk"));
        assert!(set.contains("colour"));
        assert!(set.contains("don't"));
        assert!(!set.contains("forbidden"));
        assert!(!set.contains("3"));
    }

    #[test]
    fn tolerates_a_byte_order_mark() {
        let mut set = HashSet::new();
        load_words("\u{feff}kettle\ncolour\n", false, &mut set);
        assert!(
            set.contains("kettle"),
            "the BOM must not become part of a word"
        );
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn reads_plain_word_lists_with_comments() {
        let list = "# a comment\nkettle\n\n  colour  \n";
        let mut set = HashSet::new();
        load_words(list, false, &mut set);
        assert_eq!(set.len(), 2);
        assert!(set.contains("kettle") && set.contains("colour"));
    }

    #[test]
    fn unknown_spans_cover_only_unknown_words() {
        let mut dict = dict_from(&["the", "kettle", "broke"]);
        let line = "the kettle broké badly";
        let spans = dict.unknown_spans(line);
        let chars: Vec<char> = line.chars().collect();
        let flagged: Vec<String> = spans
            .iter()
            .map(|(s, e)| chars[*s..*e].iter().collect())
            .collect();
        assert_eq!(flagged, vec!["broké", "badly"]);
    }

    #[test]
    fn personal_words_are_written_and_remembered() {
        let dir = std::env::temp_dir().join(format!("tusk-words-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("words.txt");
        let mut dict = dict_from(&["the"]);
        dict.personal = Some(path.clone());

        assert!(dict.is_unknown("hurndev"));
        assert!(dict.add_word("hurndev").expect("add"), "newly added");
        assert!(!dict.is_unknown("hurndev"));
        assert!(
            !dict.add_word("hurndev").expect("add again"),
            "already there"
        );
        let text = fs::read_to_string(&path).expect("written");
        assert!(text.contains("hurndev"));
        assert!(text.starts_with('#'), "a header explains the file");

        // Adding again appends rather than corrupting the file.
        dict.add_word("kettle").expect("add");
        let text = fs::read_to_string(&path).expect("written");
        assert_eq!(text.lines().filter(|l| !l.starts_with('#')).count(), 2);
        assert!(text.ends_with('\n'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_dictionary_explains_itself() {
        let cfg = Config {
            spellcheck: true,
            dictionary: Some("/nonexistent/definitely-not-here.txt".to_string()),
            ..Config::default()
        };
        match Dictionary::load(&cfg) {
            Status::Disabled(reason) => assert!(reason.contains("not found"), "{reason}"),
            Status::Ready(_) => panic!("should not have loaded"),
        }
    }

    #[test]
    fn loading_can_be_turned_off() {
        let cfg = Config {
            spellcheck: false,
            ..Config::default()
        };
        match Dictionary::load(&cfg) {
            Status::Disabled(reason) => assert!(reason.contains("off")),
            Status::Ready(_) => panic!("should not have loaded"),
        }
    }

    #[test]
    fn loads_from_an_explicit_path() {
        let dir = std::env::temp_dir().join(format!("tusk-dict-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("my-words.txt");
        fs::write(&path, "kettle\ncolour\n").unwrap();

        let cfg = Config {
            dictionary: Some(path.display().to_string()),
            ..Config::default()
        };
        match Dictionary::load(&cfg) {
            Status::Ready(mut dict) => {
                assert!(!dict.is_unknown("colour"));
                assert!(dict.is_unknown("kettel"));
                assert_eq!(dict.base_words, 2);
                assert!(dict.summary().contains("my-words"));
            }
            Status::Disabled(reason) => panic!("should have loaded: {reason}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
