# tusk

A small, sharp CLI journal for plain prose, with its own editor and reader.

[![CI](https://github.com/hurndev/tusk/actions/workflows/ci.yml/badge.svg)](https://github.com/hurndev/tusk/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

tusk keeps your journal in a single text file. `tusk new` opens a full-screen
editor that wraps your writing like prose and takes `Ctrl+S` to save. `tusk read`
gives the same wrapping back, with scrolling and search. No `vim`, no `less`, no
database, no daemon, no lock-in — underneath it is still a file you could read
with `cat`.

```
$ tusk new
┌─ 76 columns of prose, centred, soft-wrapped ────────────────────────────────┐
│ The kettle broke this morning, so I walked to the cafe on the corner        │
│ instead. It was busier than usual, and I ended up sharing a table with a    │
│ man who repairs accordions.                                                 │
│                                                                             │
│ I want to remember how easy it was to talk to a stranger.                   │
└─────────────────────────────────────────────────────────────────────────────┘
 new entry · 2026-04-02 08:14   ·   38 words   ·   Ln 4, Col 39   ·   modified
 Ctrl+S save   Ctrl+Q quit   Ctrl+Z undo   Ctrl+K kill line   Ctrl+←/→ word
```

## Install

Needs a recent stable Rust (2024 edition).

```sh
git clone https://github.com/hurndev/tusk
cd tusk
cargo install --path .        # installs to ~/.cargo/bin/tusk
```

Then set up once:

```sh
tusk init
```

`init` offers whatever it can detect, including the journal path from an
existing [`jrnl`](https://jrnl.sh) config if you have one.

## Writing

```sh
tusk new                       # the built-in editor
tusk new "a quick thought"     # or straight from the command line
echo "piped in" | tusk new     # or piped in
tusk edit                      # the newest entry
tusk edit 12                   # or an older one
tusk edit 2026-03-14           # the newest entry from that day
```

`tusk new` is the one command for writing: with no arguments it opens the editor,
with text it writes that text, and when something is piped in it writes whatever
arrives.

The editor is modeless — you just type.

| key | what it does |
| --- | --- |
| *any character* | insert it |
| `Enter` | new paragraph |
| `Tab` | insert four spaces |
| arrows | move, following the soft wrapping |
| `Ctrl+←` / `Ctrl+→` | move by word |
| `Home` / `End` | start / end of the line |
| `Ctrl+Home` / `Ctrl+End` | start / end of the entry |
| `PgUp` / `PgDn` | a screen at a time |
| `Ctrl+S` | **save** — writes to the journal, and you keep writing |
| `Ctrl+Q` or `Esc` | quit (asks before discarding unsaved changes) |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo |
| `Ctrl+K` | delete to the end of the line |
| `Ctrl+U` | delete to the start of the line |
| `Ctrl+W` | delete the word before the cursor |
| paste | multi-line pastes just work |

`Ctrl+S` saves *through* to your journal: after the first save the entry exists,
and further saves update the same entry. You can commit a thought early and keep
going, and you never lose a long entry to a mistyped key.

### Wrapping

Long lines are wrapped to a comfortable measure on **screen only**. Your file is
never rewritten: the line breaks you typed stay exactly as they are, because they
are part of how you wrote it. tusk shows a faint `↵` at the end of a line that
continues because of wrapping, so you can always tell a soft wrap from a real
line break.

The measure defaults to 80 columns and is centred in the window:

```sh
tusk config set prose_width 72
```

### Using your own editor

The built-in editor is the default, but any editor still works:

```sh
tusk new --external                # use $VISUAL / $EDITOR / your config
tusk new --editor "code -w"        # use this command
TUSK_EDITOR=builtin tusk new       # force the built-in editor back on
```

`editor = "vim"` in the config file does the same thing permanently.

### Editing any text file

The built-in editor is not only for entries. `tusk open PATH` edits any text
file, and `tusk open` with no path opens the journal file itself — as raw text,
which is what you want when an entry needs fixing by hand.

```sh
tusk open ~/notes/todo.md          # edit a file
tusk open ~/notes/tomorrow.md      # does not exist yet: type, Ctrl+S, it does
tusk open                          # the journal file, headers and all
```

Everything the entry editor does works here: soft wrapping, `Ctrl+Z`/`Ctrl+Y`
undo and redo, `Ctrl+K` and `Ctrl+W` to kill text, `Ctrl+A`/`Ctrl+E` for the ends
of a line, `Ctrl+S` to save, and `F2` for spellchecking. The status bar shows the
file name, the line and column, and whether the buffer differs from the disk.

Files are written the way the journal is written — to a temporary file beside
the original, then renamed over it — so a crash or a full disk cannot leave a
half-written file. If the file already existed, its permissions are kept, and a
symlink is written through rather than replaced. A file tusk creates ends with a
newline; an existing file is written back exactly as it was, so trailing bytes
stay where you left them.

Two deliberate limits: tusk will not create a directory to put a file in (make
the directory first), and it only edits UTF-8 text (a binary file is refused
rather than mangled). Backups stay a journal thing — `.tusk-backups/` is written
for the journal, not for every file you edit.

`--external` and `--editor CMD` work here too, and hand the file straight to
that editor instead of using the built-in one.

### Spellchecking

Spellchecking is **off every time you start**, and tusk never remembers whether
you had it on. Press `F2` when you want it: the words the dictionary does not
know are underlined in red, in the editor and in the reader. Press `F2` again to
put it away.

| key | what it does |
| --- | --- |
| `F2` | turn spellchecking on or off, in the editor and the reader |
| `F3` | accept the word under the cursor, and remember it (editor) |

Nothing is read from disk until the first `F2`, so starting tusk costs exactly
what it did before the feature existed.

Only the text on screen is ever looked up, and each word's verdict is
remembered, so a redraw costs a few microseconds per visible word instead of
re-reading your entry. A 40-line screenful is around 80µs — roughly 0.5% of a
frame — so it never gets in the way of typing.

The default is British English:

```sh
tusk config set dictionary en_GB          # the default
tusk config set dictionary ~/words.txt    # or point at any word list
TUSK_DICTIONARY=en_US tusk new            # or override it for one run
```

`dictionary` takes either a language tag or a path. With a tag, tusk looks in
the usual places — `/usr/share/dict/british-english`, `/usr/share/hunspell/en_GB.dic`,
`/usr/share/myspell`, and the equivalents under `~/.local` and `~/.config/tusk`:

```sh
sudo pacman -S words                 # Arch          → /usr/share/dict/british-english
sudo apt install wbritish            # Debian, Ubuntu
```

Prefer a **plain word list** such as `british-english`: it is fully inflected, so
nothing a dictionary would know is missing from it. A hunspell `.dic` works too
(its flags are ignored), but tusk does not read the companion `.aff` rules, and
some words are built only from those — a few like `every`, `many` and `has` can
end up underlined. `F3` deals with those, or install a plain list instead. On a
24,735-word journal, `british-english` flags 0.7% of words, and most of those are
names.

Either format works from any path:

```sh
tusk config set dictionary ~/wordlists/mine.txt
```

If no dictionary can be found, `F2` says which one it wanted and stays off;
`tusk config show` reports the same.

Words you vouch for — names, places, jargon — go in a personal list, which `F3`
appends to and which is read alongside the dictionary:

```sh
~/.config/tusk/words.txt             # one word per line, # for comments
```

Because this is a plain word list and not a full speller, it accepts regular
inflections of words it knows (`walked`, `running`, `carries`, `Sam's`,
`self-care`) and ignores things that are not prose: `#tags`, `@names`, acronyms
like `BBC`, and anything stuck to a digit (`10th`, `1980s`). It will not catch
every mistake, and it will occasionally flag a name. `F3` is the answer to that.

## Reading

```sh
tusk                           # the newest entry, wrapped
tusk read                      # the whole journal, in the built-in reader
tusk read 12                   # just entry 12
tusk read 2026-03              # everything from March
tusk read --from 2026-01 --to 2026-03
```

The reader is full-screen, chronological, and wrapped to the same measure as the
editor, with the date of each entry on a coloured divider. **It opens at the
bottom, on your latest entry**, and you scroll up into the past — press `g` when
you want to read from the beginning.

| key | what it does |
| --- | --- |
| `j` / `k`, `↓` / `↑` | scroll a line |
| `space` / `f`, `b` | page down / up |
| `d` / `u` | half a page |
| `g` / `G` | oldest / newest |
| `/` | search your prose; hits are highlighted |
| `n` / `N` | next / previous hit |
| `e` | edit the entry you are looking at |
| `?` | show the keys |
| `q` | quit |

`e` is the nice one: read until something needs fixing, press `e`, the editor
opens on that entry, `Ctrl+S` saves, and you are put back in the reader where you
were.

When output is not a terminal, tusk gets out of the way and prints instead, so
pipes and scripts behave:

```sh
tusk read --print | less      # wrapped text, no full-screen UI
tusk read --pager             # hand it to $PAGER instead
tusk read --raw               # exact bytes of the journal file
tusk show 1 | grep -i kettle  # piped output is never re-wrapped
```

## Finding things

```sh
tusk search kettle                       # one line per matching entry
tusk search kettle morning               # both words must appear
tusk search --any kettle morning         # either word
tusk search -r 'kettl(e|es)'             # regular expression
tusk search -s Kettle                    # case-sensitive
tusk search -t market                    # tagged #market (or @market)
tusk search --from 2026-01 --to 2026-03  # date range
tusk search kettle --full                # print the whole entries, wrapped
tusk search kettle -c                    # just the count
```

Dates accept `2026`, `2026-03`, `2026-03-14`, `today`, `yesterday`, `-7d`, `3w`,
`2m`, `1y` — anywhere `--from`/`--to` are accepted, and as entry references. Like
`grep`, `tusk search` exits non-zero when nothing matched.

### Entry numbers

Numbers count **backwards from the newest entry**, so `1` is always the thing you
wrote most recently and `tusk edit 1` needs no lookup. `tusk list` prints the same
numbers, and date prefixes work anywhere a number does:

```sh
tusk show 1                 # newest
tusk show 228               # oldest
tusk show 2026-03-14        # every entry that day
tusk show 2026-03           # every entry that month
tusk show latest            # also: oldest, first, last, newest
```

## Commands

| command | what it does |
| --- | --- |
| `new [TEXT…]` | write an entry in the editor, from text, or from stdin (alias `add`) |
| `show [ENTRY…]` | print entries, newest first; `-n N` for the newest N (alias `latest`) |
| `list` | one line per entry: `-n N`, `--reverse`, `--group`, `-v` |
| `read [ENTRY…]` | read in the built-in reader; `--print`, `--pager`, `--raw`, `--width N` |
| `edit [ENTRY]` | edit an entry in the editor; `-d WHEN` to change its timestamp |
| `delete [ENTRY]` | delete an entry; shows it first, `-y` to skip asking |
| `search QUERY…` | `-r`, `-s`, `--any`, `--full`, `-c`, `-n N`, `-t TAG`, `--from`/`--to` |
| `tags` | the tags you use, most used first |
| `stats` | words, span, writing days, streaks, busiest month |
| `open [PATH]` | edit any text file in the editor; the journal file by default (alias `file`) |
| `path` | print the journal file path in use |
| `config show\|get\|set\|unset\|edit` | settings: `journal`, `editor`, `pager`, `prose_width`, `backup` |
| `init` | choose the journal file and editor |

Most commands take `--from`, `--to`, `--tag` and `--reverse` to narrow what they
look at, and `--file PATH` works anywhere to point at another journal.

## The file format

Entries are separated by blank lines and start with a bracketed timestamp:

```
[2026-04-02 08:14:00 AM] The kettle broke this morning, so I walked to the
cafe on the corner instead.

[2026-04-05 07:40:00 PM] A quiet weekend, mostly.
```

That is exactly [`jrnl`](https://jrnl.sh)'s format, so an existing journal opens
unchanged and the file stays readable by `grep`, `less`, `cat`, Syncthing, git,
or whatever else you already trust. Entries written by tusk are written the same
way, so `jrnl` can still read them back.

Tags are `#topic`, people are `@name`; they are picked up by `tusk tags`,
`tusk search -t`, and coloured wherever they are shown.

The format is documented properly in [docs/FORMAT.md](docs/FORMAT.md).

## Configuration

`~/.config/tusk/config.toml`:

```toml
journal = "/home/you/journal.txt"   # anywhere: local disk, NAS, Syncthing folder
editor = "builtin"                  # or any external command, e.g. "vim"
prose_width = 80                    # 20-200 columns
pager = "less -R"                   # only used by `tusk read --pager`
backup = true
dictionary = "en_GB"                # a language tag, or a path to a word list
personal_words = "~/.config/tusk/words.txt"   # words you vouch for
```

The journal path is resolved in this order:

1. `--file PATH`
2. `$TUSK_FILE`, then `$TUSK_JOURNAL`
3. `journal` in the config file
4. the journal from `~/.config/jrnl/jrnl.yaml`, if present

`tusk config show` tells you which one won.

## Your words are safe

* Writes are **atomic**: tusk writes a temporary file next to the journal and
  renames it into place, so a crash or a full disk cannot leave a half-written
  journal.
* Before every change it drops a timestamped copy in
  `<journal dir>/.tusk-backups/`, keeping the last 10. `--no-backup` opts out.
* **Your line breaks are never rewritten.** The editor wraps for display only.
* Edits and deletions are **surgical**: only the target entry's byte range is
  rewritten, and adding then deleting an entry restores the file byte for byte
  (covered by tests).
* `tusk delete` shows you the entry and asks before removing it; without a
  terminal it refuses unless you pass `--yes`. Ambiguous dates (two entries on
  the same day) are rejected rather than guessed.
* Quitting the editor with unsaved changes asks first, and the terminal is
  restored even if tusk panics.

## Development

```sh
cargo test                    # 35 unit + 24 end-to-end tests
cargo clippy --all-targets
cargo fmt
cargo build --release
```

| file | what lives there |
| --- | --- |
| `src/journal.rs` | parsing, surgical edits, backups, atomic writes |
| `src/text.rs` | the prose wrapping engine (word wrap, wide characters, cursor mapping) |
| `src/buffer.rs` | the editor's text buffer: cursor, editing, undo |
| `src/ui/editor.rs` | the full-screen editor |
| `src/ui/reader.rs` | the full-screen reader |
| `src/ui/frame.rs` | terminal handling, raw mode, drawing |
| `src/query.rs` | dates and filtering |
| `src/render.rs`, `src/style.rs` | output for the non-full-screen commands |
| `src/config.rs`, `src/editor.rs`, `src/cmd.rs`, `src/cli.rs` | settings, editors, commands, arguments |

## Limits worth knowing

* One journal file at a time (use `--file` for others).
* Entries are prose; the format has no nesting, titles, or attachments.
* A body line that itself looks like `[2026-03-14 10:00] …` would be read as a
  new entry later. tusk warns you when you write one.
* Spellchecking needs a dictionary, and is off until you press `F2`. With none
  installed it stays off and says why; see [Spellchecking](#spellchecking).
* The built-in editor and reader need a terminal. In a script or a pipe, use
  `tusk new` (or pipe into it), `tusk read --print`, or `--external`.
* Encryption is not supported — that is what your disk encryption is for.

## License

GPL-3.0. See [LICENSE](LICENSE).
