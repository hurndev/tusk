//! End-to-end tests: run the real binary against throwaway journals.
//!
//! Each test gets its own directory under the system temp dir, its own
//! `TUSK_CONFIG`, and never touches a real journal.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tusk")
}

struct Sandbox {
    dir: PathBuf,
    journal: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tusk-test-{name}-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).expect("create sandbox");
        Sandbox {
            journal: dir.join("journal.txt"),
            dir,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = self.command();
        command.args(args);
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().expect("run tusk")
    }

    /// A command with the sandbox environment but no `$TUSK_FILE`, so the
    /// configured journal path wins.
    fn command(&self) -> Command {
        let mut command = Command::new(bin());
        command
            .env("TUSK_CONFIG", self.dir.join("config.toml"))
            .env("TUSK_FILE", &self.journal)
            // Keep tests hermetic: never read the real jrnl configuration.
            .env("TUSK_JRNL_CONFIG", self.dir.join("absent-jrnl.yaml"))
            .env("NO_COLOR", "1")
            .env("COLUMNS", "200")
            .env_remove("TUSK_EDITOR")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .env_remove("PAGER");
        command
    }

    fn run_configured(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(args).env_remove("TUSK_FILE");
        command.output().expect("run tusk")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "tusk {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn add(&self, date: &str, text: &str) {
        let out = self.run(&["new", "-d", date, text]);
        assert!(
            out.status.success(),
            "writing an entry failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn text(&self) -> String {
        fs::read_to_string(&self.journal).unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected to find {needle:?} in:\n{haystack}"
    );
}

#[test]
fn writes_entries_in_jrnl_format() {
    let sandbox = Sandbox::new("format");
    sandbox.add("2026-03-14 21:30", "A first thought.");
    sandbox.add("2026-03-15 08:05", "A second thought.");

    let text = sandbox.text();
    contains(&text, "[2026-03-14 09:30:00 PM] A first thought.");
    contains(&text, "[2026-03-15 08:05:00 AM] A second thought.");
    // A blank line separates entries.
    contains(&text, "A first thought.\n\n[2026-03-15");
}

#[test]
fn reads_back_existing_jrnl_files() {
    let sandbox = Sandbox::new("existing");
    let legacy = "\
[2024-03-04 09:15:00 AM] The kettle broke this morning.
It took twenty minutes to sort out.

[2024-03-05 12:00:00 AM] A quiet day.

Second paragraph.
";
    fs::write(&sandbox.journal, legacy).unwrap();

    let listing = sandbox.ok(&["list"]);
    contains(&listing, "2024-03-05 00:00");
    contains(&listing, "2 entries");

    let shown = sandbox.ok(&["show", "2"]);
    contains(&shown, "The kettle broke this morning.");
    contains(&shown, "It took twenty minutes to sort out.");
}

#[test]
fn entry_numbers_count_back_from_the_newest() {
    let sandbox = Sandbox::new("numbering");
    sandbox.add("2026-01-01 09:00", "Oldest entry.");
    sandbox.add("2026-01-02 09:00", "Middle entry.");
    sandbox.add("2026-01-03 09:00", "Newest entry.");

    let listing = sandbox.ok(&["list"]);
    let first_line = listing.lines().next().unwrap();
    contains(first_line, "1  2026-01-03 09:00");
    contains(first_line, "Newest entry.");

    contains(&sandbox.ok(&["show", "1"]), "Newest entry.");
    contains(&sandbox.ok(&["show", "3"]), "Oldest entry.");
    contains(&sandbox.ok(&["show", "2026-01-02"]), "Middle entry.");

    let too_high = sandbox.run(&["show", "4"]);
    assert!(!too_high.status.success());
    contains(
        &String::from_utf8_lossy(&too_high.stderr),
        "there is no entry 4",
    );
}

#[test]
fn bare_invocation_shows_the_latest_entry() {
    let sandbox = Sandbox::new("bare");
    sandbox.add("2026-01-01 09:00", "Only entry here.");
    contains(&sandbox.ok(&[]), "Only entry here.");
}

#[test]
fn search_matches_words_regexes_and_tags() {
    let sandbox = Sandbox::new("search");
    sandbox.add(
        "2026-01-01 09:00",
        "Walked to the market with #friends today.",
    );
    sandbox.add("2026-02-01 09:00", "Stayed home and read about beekeeping.");
    sandbox.add("2026-03-01 09:00", "Another market day.");

    let plain = sandbox.ok(&["search", "market"]);
    contains(&plain, "2 entries");
    contains(&plain, "Walked to the market");

    // All terms must match.
    let narrowed = sandbox.ok(&["search", "market", "friends"]);
    contains(&narrowed, "1 entry");
    assert!(!narrowed.contains("Another market"));

    // Any term matches with --any.
    let either = sandbox.ok(&["search", "--any", "beekeeping", "friends"]);
    contains(&either, "2 entries");

    // Regular expressions.
    let regexed = sandbox.ok(&["search", "-r", "beekeep(ing)?"]);
    contains(&regexed, "beekeeping");

    // Tags.
    let tagged = sandbox.ok(&["search", "-t", "friends"]);
    contains(&tagged, "Walked to the market");

    // A people tag and a topic tag are different namespaces.
    let people = sandbox.run(&["search", "-t", "@friends"]);
    assert!(!people.status.success());
    contains(&String::from_utf8_lossy(&people.stderr), "no matches");

    // Case sensitivity.
    let insensitive = sandbox.ok(&["search", "MARKET"]);
    contains(&insensitive, "2 entries");
    let sensitive = sandbox.run(&["search", "-s", "MARKET"]);
    assert!(!sensitive.status.success());

    // Counting.
    assert_eq!(sandbox.ok(&["search", "-c", "market"]).trim(), "2");

    // No match is exit code 1, the way grep behaves.
    let none = sandbox.run(&["search", "helicopter"]);
    assert_eq!(none.status.code(), Some(1));
    contains(&String::from_utf8_lossy(&none.stderr), "no matches");
}

#[test]
fn date_filters_and_relative_dates() {
    let sandbox = Sandbox::new("dates");
    sandbox.add("2024-05-01 09:00", "May entry.");
    sandbox.add("2025-05-01 09:00", "Next year entry.");
    sandbox.add("2026-05-01 09:00", "Latest entry.");

    contains(&sandbox.ok(&["list", "--from", "2025"]), "2 entries");
    contains(&sandbox.ok(&["list", "--to", "2024-12"]), "1 entry");
    let range = sandbox.ok(&["list", "--from", "2025-01", "--to", "2025-12"]);
    contains(&range, "Next year entry.");
    assert!(!range.contains("Latest entry"));

    let today = sandbox.ok(&["list", "--from", "today"]);
    contains(&today, "no entries match");
}

#[test]
fn editing_rewrites_only_the_targeted_entry() {
    let sandbox = Sandbox::new("edit");
    sandbox.add("2026-01-01 09:00", "Keep me.");
    sandbox.add("2026-01-02 09:00", "Change me.");
    sandbox.add("2026-01-03 09:00", "Keep me too.");

    // An "editor" that overwrites whatever it is given.
    let out = sandbox.run_with_env(
        &["edit", "2"],
        &[("TUSK_EDITOR", "printf 'Rewritten body. #edited' >")],
    );
    assert!(
        out.status.success(),
        "edit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = sandbox.text();
    contains(&text, "[2026-01-02 09:00:00 AM] Rewritten body. #edited");
    contains(&text, "Keep me.");
    contains(&text, "Keep me too.");
    assert!(!text.contains("Change me."));
    assert!(!text.contains("\n\n\n"), "blank lines got out of hand");

    // A no-op edit must not touch the file.
    let before = sandbox.text();
    let unchanged = sandbox.run_with_env(&["edit", "1"], &[("TUSK_EDITOR", "true")]);
    assert!(unchanged.status.success());
    contains(&String::from_utf8_lossy(&unchanged.stdout), "no changes");
    assert_eq!(before, sandbox.text());
}

#[test]
fn deleting_an_entry_restores_the_previous_file() {
    let sandbox = Sandbox::new("delete");
    sandbox.add("2026-01-01 09:00", "First.");
    sandbox.add("2026-01-02 09:00", "Second.");
    sandbox.add("2026-01-03 09:00", "Third.");
    let before = sandbox.text();

    sandbox.add("2026-01-04 09:00", "Temporary.");
    assert_eq!(sandbox.ok(&["list"]).matches("2026-01-0").count(), 4);

    sandbox.ok(&["delete", "1", "--yes"]);
    assert_eq!(
        before,
        sandbox.text(),
        "add-then-delete should be a perfect round trip"
    );
}

#[test]
fn delete_refuses_ambiguous_dates_and_prompts_without_yes() {
    let sandbox = Sandbox::new("delete-safe");
    sandbox.add("2026-01-01 09:00", "Morning.");
    sandbox.add("2026-01-01 21:00", "Evening.");
    let before = sandbox.text();

    let ambiguous = sandbox.run(&["delete", "2026-01-01", "--yes"]);
    assert!(!ambiguous.status.success());
    contains(
        &String::from_utf8_lossy(&ambiguous.stderr),
        "matches 2 entries",
    );
    assert_eq!(before, sandbox.text());

    // Without a terminal and without --yes, nothing is deleted.
    let unconfirmed = sandbox.run(&["delete", "1"]);
    assert!(unconfirmed.status.success());
    contains(
        &String::from_utf8_lossy(&unconfirmed.stdout),
        "nothing deleted",
    );
    assert_eq!(before, sandbox.text());

    // Dry runs leave the file alone too.
    let dry = sandbox.run(&["delete", "1", "--dry-run"]);
    assert!(dry.status.success());
    contains(&String::from_utf8_lossy(&dry.stdout), "would delete");
    assert_eq!(before, sandbox.text());
}

#[test]
fn backs_up_before_writing() {
    let sandbox = Sandbox::new("backup");
    let backups = sandbox.dir.join(".tusk-backups");

    // The very first write has nothing to back up.
    sandbox.add("2026-01-01 09:00", "Something worth keeping.");
    assert!(!backups.exists());

    // Every later write keeps a copy of what was there before.
    sandbox.add("2026-01-02 09:00", "A second thought.");
    assert_eq!(fs::read_dir(&backups).unwrap().count(), 1);
    let kept = fs::read_dir(&backups)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    contains(
        &fs::read_to_string(kept).unwrap(),
        "Something worth keeping.",
    );
    assert!(
        !fs::read_to_string(&sandbox.journal)
            .unwrap()
            .contains("backup of")
    );

    // --no-backup suppresses it.
    sandbox.ok(&[
        "new",
        "--no-backup",
        "2026-01-03 09:00",
        "No backup for this one.",
    ]);
    assert_eq!(fs::read_dir(&backups).unwrap().count(), 1);
}

#[test]
fn tags_and_stats_summarise_the_journal() {
    let sandbox = Sandbox::new("stats");
    sandbox.add("2026-01-01 09:00", "Flew with #friends and #family.");
    sandbox.add("2026-01-02 09:00", "More #friends, plus @alice.");

    let tags = sandbox.ok(&["tags"]);
    contains(&tags, "#friends");
    contains(&tags, "#family");
    contains(&tags, "@alice");

    let stats = sandbox.ok(&["stats"]);
    contains(&stats, "entries");
    contains(&stats, "2");
    contains(&stats, "span");
    contains(&stats, "2026-01-01 → 2026-01-02");
}

#[test]
fn read_emits_the_file_verbatim_with_raw() {
    let sandbox = Sandbox::new("raw");
    sandbox.add("2026-01-01 09:00", "One.");
    sandbox.add("2026-01-02 09:00", "Two.");

    let expected = sandbox.text();
    let raw = sandbox.ok(&["read", "--raw", "--no-pager"]);
    assert_eq!(raw, expected);

    let pretty = sandbox.ok(&["read", "--no-pager"]);
    contains(&pretty, "One.");
    contains(&pretty, "Two.");
    // Chronological order for reading.
    assert!(pretty.find("One.").unwrap() < pretty.find("Two.").unwrap());
}

#[test]
fn takes_entries_from_arguments_and_stdin() {
    let sandbox = Sandbox::new("input");

    // Text on the command line.
    sandbox.ok(&["new", "Quick thought."]);
    contains(&sandbox.text(), "Quick thought.");

    // Text piped in, with no editor involved.
    let mut child = Command::new(bin())
        .args(["new"])
        .env("TUSK_CONFIG", sandbox.dir.join("config.toml"))
        .env("TUSK_FILE", &sandbox.journal)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run tusk");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(b"Piped thought.\nSecond line.")
            .expect("write to stdin");
    }
    let done = child.wait_with_output().expect("wait for tusk");
    assert!(
        done.status.success(),
        "piped input failed: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    let text = sandbox.text();
    contains(&text, "Piped thought.\nSecond line.");

    // Backwards-compatible `add` alias behaves the same way.
    sandbox.ok(&["add", "Through the alias."]);
    contains(&sandbox.text(), "Through the alias.");

    // No arguments and nothing on stdin: explain, and write nothing.
    let before = sandbox.text();
    let empty = sandbox.run(&["new"]);
    assert!(!empty.status.success());
    contains(&String::from_utf8_lossy(&empty.stderr), "needs a terminal");
    contains(&String::from_utf8_lossy(&empty.stderr), "tusk new");
    assert_eq!(before, sandbox.text());
}

#[test]
fn new_uses_the_editor_and_aborts_on_empty_input() {
    let sandbox = Sandbox::new("compose");
    let out = sandbox.run_with_env(
        &["new"],
        &[("TUSK_EDITOR", "printf 'Composed prose.\\nWith #tags.' >")],
    );
    assert!(out.status.success());
    contains(&sandbox.text(), "[");
    contains(&sandbox.text(), "Composed prose.\nWith #tags.");

    // An empty buffer writes nothing.
    let before = sandbox.text();
    let aborted = sandbox.run_with_env(&["new"], &[("TUSK_EDITOR", "true")]);
    assert!(aborted.status.success());
    contains(&String::from_utf8_lossy(&aborted.stdout), "nothing written");
    assert_eq!(before, sandbox.text());
}

#[test]
fn config_commands_round_trip() {
    let sandbox = Sandbox::new("config");
    let other = sandbox.dir.join("other.txt");

    sandbox.ok(&["config", "set", "journal", other.to_str().unwrap()]);
    let raw = fs::read_to_string(sandbox.dir.join("config.toml")).unwrap();
    contains(&raw, other.to_str().unwrap());

    // With no environment override, the configured path is what tusk uses.
    let configured = sandbox.run_configured(&["config", "get", "journal"]);
    assert!(configured.status.success());
    assert_eq!(
        String::from_utf8_lossy(&configured.stdout).trim(),
        other.to_str().unwrap()
    );
    let path = sandbox.run_configured(&["path"]);
    assert_eq!(
        String::from_utf8_lossy(&path.stdout).trim(),
        other.to_str().unwrap()
    );

    sandbox.ok(&["config", "set", "editor", "nano"]);
    let editor = sandbox.run_configured(&["config", "get", "editor"]);
    assert_eq!(String::from_utf8_lossy(&editor.stdout).trim(), "nano");

    sandbox.ok(&["config", "set", "backup", "off"]);
    let backup = sandbox.run_configured(&["config", "get", "backup"]);
    assert_eq!(String::from_utf8_lossy(&backup.stdout).trim(), "false");

    sandbox.ok(&["config", "unset", "editor"]);

    let show = sandbox.ok(&["config", "show"]);
    contains(&show, "config");
    contains(&show, "other.txt");
    contains(&show, "from $TUSK_FILE");

    let bad_key = sandbox.run(&["config", "set", "nonsense", "1"]);
    assert!(!bad_key.status.success());
    contains(&String::from_utf8_lossy(&bad_key.stderr), "unknown setting");
}

#[test]
fn long_entries_are_wrapped_by_the_reader_and_preserved_when_piped() {
    let sandbox = Sandbox::new("prose");
    let long = "Prose should wrap at a comfortable measure rather than running the \
                full width of whatever terminal happens to be in front of you. \
                This sentence exists purely to be longer than eighty columns.";
    let long = long.split_whitespace().collect::<Vec<_>>().join(" ");
    sandbox.add("2026-01-01 09:00", &long);

    // The reader's plain output wraps to the configured prose width.
    let printed = sandbox.ok(&["read", "--print"]);
    assert!(printed.contains("Prose should wrap"), "{printed}");
    for line in printed.lines() {
        assert!(
            line.chars().count() <= 80,
            "line is wider than the prose width: {line:?}"
        );
    }
    assert!(
        printed.lines().filter(|l| l.contains("wrap")).count() >= 1,
        "the wrapped text should survive the round trip"
    );

    // A narrower prose width really does wrap more tightly.
    sandbox.ok(&["config", "set", "prose_width", "40"]);
    let narrow = sandbox.ok(&["read", "--print"]);
    for line in narrow.lines() {
        assert!(
            line.chars().count() <= 40,
            "line is wider than 40 columns: {line:?}"
        );
    }
    assert!(
        narrow.lines().count() > printed.lines().count(),
        "a narrower measure should produce more lines"
    );

    // Piped output from `show` keeps your own line breaks, so scripts see the
    // journal as it is stored.
    let shown = sandbox.ok(&["show", "1"]);
    assert!(
        shown.lines().any(|l| l.chars().count() > 100),
        "piped output should not be re-wrapped: {shown}"
    );
}

#[test]
fn read_accepts_entry_arguments() {
    let sandbox = Sandbox::new("read-args");
    sandbox.add("2026-01-01 09:00", "The oldest entry.");
    sandbox.add("2026-01-02 09:00", "A middle entry.");
    sandbox.add("2026-01-03 09:00", "The newest entry.");

    let one = sandbox.ok(&["read", "--print", "1"]);
    contains(&one, "The newest entry.");
    assert!(!one.contains("The oldest entry."));

    let range = sandbox.ok(&["read", "--print", "2026-01-01", "2"]);
    contains(&range, "The oldest entry.");
    contains(&range, "A middle entry.");
    assert!(!range.contains("The newest entry."));
}

#[test]
fn the_built_in_editor_is_the_default_and_says_so_without_a_terminal() {
    let sandbox = Sandbox::new("builtin");
    // No terminal here, no external editor configured: explain the options
    // rather than hanging or writing something empty.
    let out = sandbox.run(&["new"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    contains(&stderr, "built-in editor needs a terminal");
    contains(&stderr, "| tusk new");
    assert!(
        !sandbox.journal.exists(),
        "nothing should have been written"
    );

    // --builtin asks for the same thing explicitly.
    let explicit = sandbox.run(&["new", "--builtin"]);
    assert!(!explicit.status.success());
    contains(
        &String::from_utf8_lossy(&explicit.stderr),
        "built-in editor needs a terminal",
    );

    // TUSK_EDITOR=builtin means the built-in editor, not a command called
    // "builtin".
    let via_env = sandbox.run_with_env(&["new"], &[("TUSK_EDITOR", "builtin")]);
    assert!(!via_env.status.success());
    contains(
        &String::from_utf8_lossy(&via_env.stderr),
        "built-in editor needs a terminal",
    );
}

#[test]
fn editor_overrides_are_honoured() {
    let sandbox = Sandbox::new("editor-flags");

    // --editor names an external command.
    let out = sandbox.run(&["new", "--editor", "printf 'Flagged entry.' >"]);
    assert!(
        out.status.success(),
        "--editor should have been used: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    contains(&sandbox.text(), "Flagged entry.");

    // --editor builtin means tusk's own editor, which needs a terminal.
    let builtin = sandbox.run(&["new", "--editor", "builtin"]);
    assert!(!builtin.status.success());
    contains(
        &String::from_utf8_lossy(&builtin.stderr),
        "needs a terminal",
    );

    // A configured external editor is used when the built-in one cannot run.
    sandbox.ok(&["config", "set", "editor", "printf 'From the config.' >"]);
    let from_config = sandbox.run(&["new"]);
    assert!(
        from_config.status.success(),
        "{}",
        String::from_utf8_lossy(&from_config.stderr)
    );
    contains(&sandbox.text(), "From the config.");

    // ...but --builtin still wins over it.
    let forced = sandbox.run(&["new", "--builtin"]);
    assert!(!forced.status.success());
}

#[test]
fn prospecting_the_prose_width_setting() {
    let sandbox = Sandbox::new("prose-width");
    sandbox.ok(&["config", "set", "prose_width", "72"]);
    assert_eq!(sandbox.ok(&["config", "get", "prose_width"]).trim(), "72");
    assert_eq!(sandbox.ok(&["config", "get", "width"]).trim(), "72");

    let too_narrow = sandbox.run(&["config", "set", "prose_width", "5"]);
    assert!(!too_narrow.status.success());
    contains(
        &String::from_utf8_lossy(&too_narrow.stderr),
        "between 20 and 200",
    );

    let not_a_number = sandbox.run(&["config", "set", "width", "wide"]);
    assert!(!not_a_number.status.success());

    sandbox.ok(&["config", "unset", "prose_width"]);
    assert_eq!(sandbox.ok(&["config", "get", "prose_width"]).trim(), "80");

    let show = sandbox.ok(&["config", "show"]);
    contains(&show, "prose");
}

#[test]
fn init_is_non_interactive_with_yes() {
    let sandbox = Sandbox::new("init");
    let out = sandbox.run(&["init", "-y", "-f", sandbox.journal.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    contains(&stdout, "configured tusk");
    contains(&stdout, "0 entries so far");

    // The journal can now be used without --file or $TUSK_FILE.
    let mut command = Command::new(bin());
    let out = command
        .args(["new", "After init."])
        .env("TUSK_CONFIG", sandbox.dir.join("config.toml"))
        .env_remove("TUSK_FILE")
        .env("NO_COLOR", "1")
        .output()
        .expect("run tusk");
    assert!(out.status.success());
    contains(&sandbox.text(), "After init.");
}

#[test]
fn unconfigured_tusk_explains_itself() {
    let sandbox = Sandbox::new("unconfigured");
    let mut command = Command::new(bin());
    let out = command
        .args(["list"])
        .env("TUSK_CONFIG", sandbox.dir.join("config.toml"))
        .env_remove("TUSK_FILE")
        .env_remove("TUSK_JOURNAL")
        .env("TUSK_JRNL_CONFIG", sandbox.dir.join("does-not-exist.yaml"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run tusk");
    assert!(!out.status.success());
    contains(&String::from_utf8_lossy(&out.stderr), "tusk init");
}

#[test]
fn empty_journal_is_a_friendly_non_error() {
    let sandbox = Sandbox::new("empty");
    let listing = sandbox.ok(&["list"]);
    contains(&listing, "the journal is empty");
    contains(&sandbox.ok(&["show"]), "the journal is empty");
    contains(&sandbox.ok(&["latest"]), "the journal is empty");
}

#[test]
fn creates_missing_parent_directories() {
    let sandbox = Sandbox::new("mkdir");
    let nested = sandbox.dir.join("deep").join("down").join("journal.txt");
    let out = sandbox.run(&["--file", nested.to_str().unwrap(), "new", "Into the deep."]);
    assert!(
        out.status.success(),
        "expected the parent dir to be created: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(nested.is_file());
    contains(&fs::read_to_string(&nested).unwrap(), "Into the deep.");
}
