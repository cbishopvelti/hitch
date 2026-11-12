//! Reading and writing the HITCH.md format.
//!
//! A notes file is an optional preamble (usually `# Hitch notes`) followed by
//! one H2 section per note:
//!
//! ```markdown
//! ## Parser bug
//! <!-- hitch id=01JC8Z9K2B7M file=src/parser.rs created=2026-11-11 -->
//! Fails on empty front matter.
//! ```
//!
//! The metadata comment is optional, so a section written by hand still
//! counts as a note. Such a note gets an id derived from its heading, which
//! stays the same from one load to the next, and the id is only written to
//! the file if Hitch edits that note.
//!
//! The file is committed with the project, so diffs must stay small:
//! [`NotesFile::render`] writes every section Hitch did not change back
//! byte for byte, and only re-renders the notes that were added or edited.

use std::path::{Path, PathBuf};

use crate::Note;
use crate::id;

/// Heading written at the top of a new notes file.
pub const DEFAULT_PREAMBLE: &str = "# Hitch notes\n";

/// A parsed notes file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotesFile {
    /// Everything before the first note, kept verbatim.
    preamble: String,
    sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    note: Note,
    /// The note as parsed and its exact text, for sections read from disk.
    /// While `note` still equals the parsed note, `raw` is written back.
    original: Option<(Note, String)>,
    /// The section ended the file when it was parsed.
    was_last: bool,
}

impl NotesFile {
    /// An empty notes file with the default `# Hitch notes` heading.
    pub fn new() -> Self {
        Self::with_preamble(DEFAULT_PREAMBLE)
    }

    /// An empty notes file with the given preamble, such as a heading naming
    /// the app a fallback file is for.
    pub fn with_preamble(preamble: impl Into<String>) -> Self {
        Self {
            preamble: preamble.into(),
            sections: Vec::new(),
        }
    }

    /// Parse a notes file. Never fails: anything that isn't a note is kept
    /// as preamble or note body.
    pub fn parse(text: &str) -> Self {
        let mut preamble = String::new();
        // (heading line, following lines with their fence state)
        let mut chunks: Vec<(String, Vec<(String, bool)>)> = Vec::new();
        let mut fence: Option<Fence> = None;

        for line in text.split_inclusive('\n') {
            let in_fence = fence.is_some();
            match &fence {
                Some(open) => {
                    if open.closed_by(line) {
                        fence = None;
                    }
                }
                None => {
                    if let Some(open) = Fence::opened_by(line) {
                        fence = Some(open);
                    } else if h2_title(line).is_some() {
                        chunks.push((line.to_owned(), Vec::new()));
                        continue;
                    }
                }
            }
            match chunks.last_mut() {
                Some((_, lines)) => lines.push((line.to_owned(), in_fence)),
                None => preamble.push_str(line),
            }
        }

        let mut seen_titles: Vec<String> = Vec::new();
        let mut sections = Vec::with_capacity(chunks.len());
        for (heading, lines) in chunks {
            let mut raw = heading.clone();
            for (l, _) in &lines {
                raw.push_str(l);
            }
            let title = h2_title(&heading).unwrap_or_default();
            let mut rest = lines.as_slice();
            // The metadata comment is the first non-blank line, if it is one.
            let first = rest.iter().position(|(l, _)| !l.trim().is_empty());
            let meta = first.and_then(|i| parse_meta(&rest[i].0).map(|m| (i, m)));
            let mut note = Note {
                title,
                ..Note::default()
            };
            if let Some((i, meta)) = meta {
                rest = &rest[i + 1..];
                apply_meta(&mut note, meta);
            }
            note.body = body_from_lines(rest);
            if note.id.is_none() {
                let nth = seen_titles.iter().filter(|t| **t == note.title).count();
                note.id = Some(id::derived(&note.title, nth));
            }
            seen_titles.push(note.title.clone());
            sections.push(Section {
                original: Some((note.clone(), raw)),
                note,
                was_last: false,
            });
        }
        if let Some(last) = sections.last_mut() {
            last.was_last = true;
        }

        Self { preamble, sections }
    }

    /// Write the file back out. Unchanged sections keep their exact text.
    pub fn render(&self) -> String {
        let mut out = self.preamble.clone();
        let count = self.sections.len();
        for (i, section) in self.sections.iter().enumerate() {
            let last = i + 1 == count;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            let unchanged = section
                .original
                .as_ref()
                .filter(|(orig, _)| *orig == section.note)
                .map(|(_, raw)| raw.as_str());
            match unchanged {
                Some(raw) if last && !section.was_last => {
                    // It used to be followed by a note that is gone now:
                    // drop the blank lines that separated them.
                    out.push_str(raw.trim_end_matches(['\n', '\r']));
                    out.push_str(if raw.contains("\r\n") { "\r\n" } else { "\n" });
                }
                Some(raw) => out.push_str(raw),
                None => {
                    if !out.is_empty() && !out.ends_with("\n\n") && !out.ends_with("\n\r\n") {
                        out.push('\n');
                    }
                    out.push_str(&render_note(&section.note));
                    if !last {
                        out.push('\n');
                    }
                }
            }
        }
        if count > 0 && !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// The preamble: everything before the first note.
    pub fn preamble(&self) -> &str {
        &self.preamble
    }

    /// The notes in file order.
    pub fn notes(&self) -> impl ExactSizeIterator<Item = &Note> + '_ {
        self.sections.iter().map(|s| &s.note)
    }

    /// Number of notes.
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// True when the file holds no notes.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The note with this id.
    pub fn get(&self, id: &str) -> Option<&Note> {
        self.position(id).map(|i| &self.sections[i].note)
    }

    /// The note with this id, to change in place.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Note> {
        self.position(id).map(|i| &mut self.sections[i].note)
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| s.note.id.as_deref() == Some(id))
    }

    /// Insert or replace a note by id, and return the stored note.
    ///
    /// A note with no id gets a new one, and a new note with no creation
    /// date gets today's. A note whose id isn't in the file is appended.
    pub fn upsert(&mut self, mut note: Note) -> &Note {
        let existing = note.id.as_deref().and_then(|id| self.position(id));
        if note.created.is_none() {
            // Keep the stored date; only a new note is dated today. (A
            // hand-written note has no date, and Hitch doesn't invent one.)
            note.created = match existing {
                Some(i) => self.sections[i].note.created.clone(),
                None => Some(id::today()),
            };
        }
        match existing {
            Some(i) => {
                self.sections[i].note = note;
                &self.sections[i].note
            }
            None => {
                if note.id.is_none() {
                    note.id = Some(self.fresh_id());
                }
                self.sections.push(Section {
                    note,
                    original: None,
                    was_last: false,
                });
                &self.sections.last().expect("just pushed").note
            }
        }
    }

    /// Remove a note by id and return it.
    pub fn remove(&mut self, id: &str) -> Option<Note> {
        self.position(id).map(|i| self.sections.remove(i).note)
    }

    fn fresh_id(&self) -> String {
        loop {
            let id = id::generate();
            if self.position(&id).is_none() {
                return id;
            }
        }
    }
}

/// Render one note in the canonical format, ending with a newline.
pub fn render_note(note: &Note) -> String {
    let mut out = String::from("##");
    let title = one_line(&note.title);
    if !title.is_empty() {
        out.push(' ');
        out.push_str(&title);
    }
    out.push('\n');
    out.push_str(&render_meta(note));
    out.push('\n');
    let body = escape_body(note.body.trim_end());
    if !body.is_empty() {
        out.push_str(&body);
        out.push('\n');
    }
    out
}

fn one_line(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // A title that ends in `#` would lose it as a closing sequence.
    if joined.ends_with('#') {
        format!("{joined} ##")
    } else {
        joined
    }
}

// --- headings and fences -------------------------------------------------

/// The title of an ATX H2 heading line, or `None` if the line isn't one.
fn h2_title(line: &str) -> Option<String> {
    let line = line.trim_end_matches(['\n', '\r']);
    let rest = strip_indent(line)?;
    let after = rest.strip_prefix("##")?;
    if !(after.is_empty() || after.starts_with([' ', '\t'])) {
        return None;
    }
    let mut title = after.trim();
    // Optional closing sequence: spaces then only `#`s.
    let without_hashes = title.trim_end_matches('#');
    if without_hashes.is_empty() {
        title = "";
    } else if without_hashes.len() != title.len() && without_hashes.ends_with([' ', '\t']) {
        title = without_hashes.trim_end();
    }
    Some(title.to_owned())
}

/// Strip up to three leading spaces; `None` if the line is indented code.
fn strip_indent(line: &str) -> Option<&str> {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    if spaces > 3 {
        None
    } else {
        Some(&line[spaces..])
    }
}

#[derive(Debug, Clone, Copy)]
struct Fence {
    ch: char,
    len: usize,
}

impl Fence {
    fn opened_by(line: &str) -> Option<Self> {
        let rest = strip_indent(line.trim_end_matches(['\n', '\r']))?;
        let ch = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let len = rest.len() - rest.trim_start_matches(ch).len();
        if len < 3 {
            return None;
        }
        if ch == '`' && rest[len..].contains('`') {
            return None;
        }
        Some(Self { ch, len })
    }

    fn closed_by(&self, line: &str) -> bool {
        let Some(rest) = strip_indent(line.trim_end_matches(['\n', '\r'])) else {
            return false;
        };
        let len = rest.len() - rest.trim_start_matches(self.ch).len();
        len >= self.len && rest[len..].trim().is_empty()
    }
}

// --- body escaping -------------------------------------------------------
//
// A body line that looks like an H2 would split the note in two when the
// file is read back. Hitch writes such lines with one more leading
// backslash (`\## Heading`, which Markdown shows as `## Heading`) and drops
// one backslash when reading. Lines that already start with backslashes get
// one more, so the round trip is exact.

fn escapable(line: &str) -> Option<usize> {
    let rest = strip_indent(line.trim_end_matches(['\n', '\r']))?;
    let indent = line.len() - line.trim_start_matches(' ').len();
    let after = rest.trim_start_matches('\\');
    h2_title(after).map(|_| indent)
}

fn escape_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<Fence> = None;
    for line in body.split_inclusive('\n') {
        match &fence {
            Some(open) => {
                if open.closed_by(line) {
                    fence = None;
                }
                out.push_str(line);
            }
            None => {
                if let Some(open) = Fence::opened_by(line) {
                    fence = Some(open);
                    out.push_str(line);
                } else if let Some(indent) = escapable(line) {
                    out.push_str(&line[..indent]);
                    out.push('\\');
                    out.push_str(&line[indent..]);
                } else {
                    out.push_str(line);
                }
            }
        }
    }
    // An unclosed fence would swallow every note after this one.
    if let Some(open) = fence {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.extend(std::iter::repeat_n(open.ch, open.len));
    }
    out
}

fn body_from_lines(lines: &[(String, bool)]) -> String {
    let mut body = String::new();
    for (line, in_fence) in lines {
        let starts_escaped = line.trim_start_matches(' ').starts_with('\\');
        match escapable(line) {
            Some(indent) if !in_fence && starts_escaped => {
                body.push_str(&line[..indent]);
                body.push_str(&line[indent + 1..]);
            }
            _ => body.push_str(line),
        }
    }
    let trimmed = body.trim_end();
    let start = trimmed
        .split_inclusive('\n')
        .take_while(|l| l.trim().is_empty())
        .map(str::len)
        .sum::<usize>();
    trimmed[start..].replace("\r\n", "\n")
}

// --- metadata comment ----------------------------------------------------

/// Parse `<!-- hitch key=value ... -->` into its key/value pairs.
fn parse_meta(line: &str) -> Option<Vec<(String, String)>> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();
    let rest = inner.strip_prefix("hitch")?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let mut pairs = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(c) = chars.next_if(|c| *c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        let mut value = String::new();
        if chars.next_if_eq(&'=').is_some() {
            if chars.next_if_eq(&'"').is_some() {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(n) = chars.next() {
                                value.push(n);
                            }
                        }
                        c => value.push(c),
                    }
                }
            } else {
                while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                    value.push(c);
                }
            }
        }
        if !key.is_empty() {
            pairs.push((key, value));
        }
    }
    Some(pairs)
}

fn apply_meta(note: &mut Note, meta: Vec<(String, String)>) {
    for (key, value) in meta {
        match key.as_str() {
            "id" if note.id.is_none() && !value.is_empty() => note.id = Some(value),
            "file" if !value.is_empty() => note.files.push(PathBuf::from(value)),
            "created" if note.created.is_none() && !value.is_empty() => note.created = Some(value),
            _ => note.extra.push((key, value)),
        }
    }
}

fn render_meta(note: &Note) -> String {
    let mut out = String::from("<!-- hitch");
    let mut push = |key: &str, value: &str| {
        out.push(' ');
        out.push_str(key);
        out.push('=');
        out.push_str(&quote(value));
    };
    if let Some(id) = &note.id {
        push("id", id);
    }
    for file in &note.files {
        push("file", &path_tag(file));
    }
    if let Some(created) = &note.created {
        push("created", created);
    }
    for (key, value) in &note.extra {
        push(key, value);
    }
    out.push_str(" -->");
    out
}

/// A file tag as written in the metadata: forward slashes on every platform.
pub(crate) fn path_tag(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '>' | '='));
    if plain {
        return value.to_owned();
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' | '\\' | '>' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = "# Hitch notes\n\n## Dev setup\n<!-- hitch id=01JC8Z6Q3N4V created=2026-11-11 -->\nRun `cargo run`; the first build takes a few minutes.\n\n## Parser bug\n<!-- hitch id=01JC8Z9K2B7M file=src/parser.rs created=2026-11-11 -->\nFails on empty front matter.\n";

    #[test]
    fn parses_the_spec_example() {
        let f = NotesFile::parse(SPEC_EXAMPLE);
        assert_eq!(f.preamble(), "# Hitch notes\n\n");
        let notes: Vec<_> = f.notes().collect();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].id.as_deref(), Some("01JC8Z6Q3N4V"));
        assert_eq!(notes[0].title, "Dev setup");
        assert_eq!(
            notes[0].body,
            "Run `cargo run`; the first build takes a few minutes."
        );
        assert!(notes[0].files.is_empty());
        assert_eq!(notes[1].files, vec![PathBuf::from("src/parser.rs")]);
        assert_eq!(notes[1].created.as_deref(), Some("2026-11-11"));
    }

    #[test]
    fn canonical_round_trip_is_exact() {
        assert_eq!(NotesFile::parse(SPEC_EXAMPLE).render(), SPEC_EXAMPLE);
        let mut f = NotesFile::parse(SPEC_EXAMPLE);
        let mut n = f.get("01JC8Z9K2B7M").unwrap().clone();
        n.body = "Fails on empty front matter.".into();
        f.upsert(n);
        assert_eq!(
            f.render(),
            SPEC_EXAMPLE,
            "an unchanged edit writes nothing new"
        );
    }

    #[test]
    fn hand_written_file_survives_untouched() {
        let text = "Intro text\r\n\r\n##   Odd  spacing ##\r\nbody\r\n## Second\nno blank line before me\n\n\n";
        let f = NotesFile::parse(text);
        assert_eq!(f.len(), 2);
        assert_eq!(f.notes().next().unwrap().title, "Odd  spacing");
        assert_eq!(f.render(), text);
    }

    #[test]
    fn hand_written_notes_get_stable_ids() {
        let text = "## Same\none\n\n## Same\ntwo\n";
        let a = NotesFile::parse(text);
        let b = NotesFile::parse(text);
        let ids: Vec<_> = a.notes().map(|n| n.id.clone().unwrap()).collect();
        assert_ne!(ids[0], ids[1]);
        assert_eq!(
            ids,
            b.notes().map(|n| n.id.clone().unwrap()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn editing_one_note_only_changes_that_section() {
        let mut f = NotesFile::parse(SPEC_EXAMPLE);
        f.get_mut("01JC8Z6Q3N4V").unwrap().body = "Run `just dev`.".into();
        let out = f.render();
        assert_eq!(
            out,
            SPEC_EXAMPLE.replace(
                "Run `cargo run`; the first build takes a few minutes.",
                "Run `just dev`."
            )
        );
    }

    #[test]
    fn editing_a_hand_written_note_adds_metadata() {
        let mut f = NotesFile::parse("## Todo\nfix it\n## Other\nkeep\n");
        let id = f.notes().next().unwrap().id.clone().unwrap();
        f.get_mut(&id).unwrap().body = "fixed".into();
        let out = f.render();
        assert_eq!(
            out,
            format!("## Todo\n<!-- hitch id={id} -->\nfixed\n\n## Other\nkeep\n")
        );
        let again = NotesFile::parse(&out);
        assert_eq!(again.get(&id).unwrap().body, "fixed");
    }

    #[test]
    fn appending_to_a_file_without_trailing_newline() {
        let mut f = NotesFile::parse("# Hitch notes\n\n## A\ntext");
        f.upsert(Note {
            id: Some("X".into()),
            title: "B".into(),
            body: "more".into(),
            created: Some("2026-11-11".into()),
            ..Note::default()
        });
        assert_eq!(
            f.render(),
            "# Hitch notes\n\n## A\ntext\n\n## B\n<!-- hitch id=X created=2026-11-11 -->\nmore\n"
        );
    }

    #[test]
    fn new_file_layout() {
        let mut f = NotesFile::new();
        let a = f.upsert(Note::new("A", "one")).clone();
        assert!(a.id.is_some() && a.created.is_some());
        f.upsert(Note {
            id: Some("B1".into()),
            title: "B".into(),
            created: Some("2026-11-12".into()),
            ..Note::default()
        });
        let out = f.render();
        let id = a.id.unwrap();
        let created = a.created.unwrap();
        assert_eq!(
            out,
            format!(
                "# Hitch notes\n\n## A\n<!-- hitch id={id} created={created} -->\none\n\n## B\n<!-- hitch id=B1 created=2026-11-12 -->\n"
            )
        );
        assert_eq!(NotesFile::parse(&out).render(), out);
    }

    #[test]
    fn removing_the_last_note_keeps_a_single_trailing_newline() {
        let mut f = NotesFile::parse(SPEC_EXAMPLE);
        f.remove("01JC8Z9K2B7M").unwrap();
        assert_eq!(
            f.render(),
            "# Hitch notes\n\n## Dev setup\n<!-- hitch id=01JC8Z6Q3N4V created=2026-11-11 -->\nRun `cargo run`; the first build takes a few minutes.\n"
        );
        let mut f = NotesFile::parse(SPEC_EXAMPLE);
        f.remove("01JC8Z6Q3N4V").unwrap();
        assert_eq!(
            f.render(),
            "# Hitch notes\n\n## Parser bug\n<!-- hitch id=01JC8Z9K2B7M file=src/parser.rs created=2026-11-11 -->\nFails on empty front matter.\n"
        );
    }

    #[test]
    fn headings_inside_code_fences_are_body() {
        let text =
            "## Snippet\n```md\n## not a note\n```\n\n~~~~\n## nor this\n~~~\n~~~~\n## Real\nx\n";
        let f = NotesFile::parse(text);
        let titles: Vec<_> = f.notes().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["Snippet", "Real"]);
        assert!(f.notes().next().unwrap().body.contains("## nor this"));
        assert_eq!(f.render(), text);
    }

    #[test]
    fn h3_and_indented_code_are_not_notes() {
        let f = NotesFile::parse("## A\n### sub\n    ## code\n#hashtag\n");
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn body_headings_are_escaped_and_restored() {
        let mut f = NotesFile::new();
        let body = "intro\n## looks like a note\n\\## already escaped\n```\n## in code\n```";
        let id = f.upsert(Note::new("T", body)).id.clone().unwrap();
        let out = f.render();
        assert!(out.contains("\n\\## looks like a note\n"));
        assert!(out.contains("\n\\\\## already escaped\n"));
        assert!(out.contains("\n## in code\n"));
        let back = NotesFile::parse(&out);
        assert_eq!(back.len(), 1);
        assert_eq!(back.get(&id).unwrap().body, body);
    }

    #[test]
    fn unclosed_fence_in_body_is_closed() {
        let mut f = NotesFile::new();
        f.upsert(Note::new("A", "```sh\nls"));
        f.upsert(Note::new("B", "b"));
        let back = NotesFile::parse(&f.render());
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn metadata_quoting_round_trips() {
        let mut f = NotesFile::new();
        let note = Note {
            id: Some("Q1".into()),
            title: "Quoted".into(),
            body: "b".into(),
            files: vec!["docs/my file.md".into(), "src/a.rs".into()],
            created: Some("2026-11-11".into()),
            extra: vec![
                ("weird".into(), "a \"b\" --> c\\d".into()),
                ("empty".into(), String::new()),
            ],
        };
        f.upsert(note.clone());
        let out = f.render();
        assert!(out.contains("file=\"docs/my file.md\" file=src/a.rs"));
        assert!(!out.lines().nth(3).unwrap().contains("--> c"));
        let back = NotesFile::parse(&out);
        assert_eq!(back.get("Q1").unwrap(), &note);
    }

    #[test]
    fn unknown_keys_and_blank_line_before_metadata() {
        let text = "## A\n\n<!-- hitch id=A1 pinned color=red -->\nbody\n";
        let f = NotesFile::parse(text);
        let n = f.get("A1").unwrap();
        assert_eq!(
            n.extra,
            vec![
                ("pinned".to_string(), String::new()),
                ("color".to_string(), "red".to_string())
            ]
        );
        assert_eq!(n.body, "body");
        assert_eq!(f.render(), text);
    }

    #[test]
    fn titles_are_kept_on_one_line() {
        let mut f = NotesFile::new();
        f.upsert(Note::new("two\nlines #", "b"));
        let back = NotesFile::parse(&f.render());
        assert_eq!(back.notes().next().unwrap().title, "two lines #");
    }

    #[test]
    fn empty_title() {
        let f = NotesFile::parse("##\nbody\n## ##\nmore\n");
        assert_eq!(f.len(), 2);
        assert!(f.notes().all(|n| n.title.is_empty()));
    }
}
