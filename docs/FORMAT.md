# The tusk file format

A tusk journal is a plain UTF-8 text file. There is no database, no index, and no
sidecar metadata. Everything tusk knows is in the file, and anything that can
read text can read it.

## Entries

An entry begins with a timestamp in square brackets at the start of a line, and
continues until the next such line:

```
[2026-04-02 08:14:00 AM] The kettle broke this morning, so I walked to the
cafe on the corner instead.

[2026-04-05 07:40:00 PM] A quiet weekend, mostly.

Second paragraph of the same entry.
```

So:

* The **header** is `[` + a timestamp + `]`, on its own line start.
* The **body** is whatever follows the `]` on that line, plus every following
  line up to the next header.
* A single space after `]` is part of the syntax and is not stored.
* A blank line between entries is conventional, not required. Blank lines
  *inside* an entry are preserved exactly.
* A body may be empty (a header with nothing after it). tusk keeps such entries
  and shows them as `(empty)`.

## Timestamps

The canonical form tusk writes is:

```
[2026-04-02 08:14:00 AM]
```

which is `%Y-%m-%d %I:%M:%S %p` — a local time, 12-hour clock, zero-padded,
uppercase meridiem. This is the same layout `jrnl` v4 uses by default
(`timeformat: '%F %r'`), which is what makes the two tools interchangeable.

For reading, tusk is more forgiving. These are all accepted:

```
[2026-04-02 08:14:00 AM]
[2026-04-02 8:14 am]
[2026-04-02 08:14]
[2026-04-02 08:14:00]
[2026-04-02T08:14:00]
[2026-04-02]
```

A line that begins with `[` but does not contain a parsable timestamp is treated
as ordinary prose, not as an entry header.

### Timestamps are local time

tusk records the local time of the machine that wrote the entry; there is no
timezone stored in the file. Sorting and date filtering use those wall-clock
values directly. If you journal across timezones, entries are ordered by the time
you wrote them, not by absolute instant.

## Tags

Two kinds of tag are recognised inside entry bodies:

* `#topic` — subjects, e.g. `#garden`, `#self-care`
* `@name` — people, e.g. `@alice`

A tag starts with `#` or `@` followed by a letter, digit, or underscore, and
continues with letters, digits, `_`, `-`, or `/`. A tag only counts when the
character before it is not a letter, digit, or another tag character — so email
addresses (`me@example.com`) and mid-word hashes (`a#b`) are left alone.

`#` and `@` are separate namespaces: `-t garden` matches `#garden`, and
`-t @garden` matches only `@garden`.

Tags are derived, never stored separately. They are computed from the body each
time they are needed.

## What tusk will not do to your file

* **It will not re-wrap your text.** The editor and reader wrap long lines for
  display only. The bytes you typed are the bytes on disk.
* **It will not rewrite the whole file to edit one entry.** `edit` and `delete`
  replace the byte range of the target entry, so the rest of the journal is
  untouched, byte for byte.
* **It will not leave a half-written file.** Writes go to a temporary file in the
  same directory, which is then renamed over the journal.
* **It will not write anywhere else** except `<journal dir>/.tusk-backups/` (a
  timestamped copy before each change, 10 kept) and tusk's own temporary files
  while an external editor is open.

## Append, edit and delete

* **Append** — the trailing whitespace of the file is trimmed, then `\n\n` and the
  new entry, then a single `\n`. The file always ends with exactly one newline.
* **Edit** — the byte range from the header's `[` to the last byte of the body is
  replaced with the new header and body. Separators around other entries are not
  disturbed.
* **Delete** — the entry's byte range is removed, and the join is tidied so that
  the entries either side are separated by exactly one blank line.

The invariant the test suite checks: appending an entry and then deleting it
leaves the file byte-for-byte identical to how it started.

## Interoperability

Because the format is `jrnl`'s, a tusk journal can be read by `jrnl`, and a jrnl
journal can be read and appended to by tusk. Other things that work: `grep`,
`rg`, `less`, `cat`, `wc`, git, `rsync`, Syncthing, Dropbox, and any editor.

Two things to be careful about when mixing tools:

* Don't edit the same journal with two tools at once. Both write whole files (or
  whole entries) and the last writer wins.
* Don't put secrets in a journal you sync or commit. There is no encryption.
