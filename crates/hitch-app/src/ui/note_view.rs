//! Rendering notes as Markdown, and the editable text of a note.
//!
//! [`NoteViewer`] draws a note's body with iced's Markdown widget. It
//! overrides two things:
//!
//! - **Task lists**: checkboxes are clickable and toggle the matching
//!   `- [ ]` marker in the note's source ([`toggle_task`]).
//! - **Code blocks**: every block is numbered in panel order and shows Copy
//!   and Send to Zellij buttons on hover (spec F12, F13). Keys 1 to 9 use
//!   the same numbers: [`code_blocks`] lists the same blocks, in the same
//!   order, as raw text.

use std::cell::Cell;
use std::ops::Range;

use iced::widget::markdown::{self, Bullet, Item, Settings, Uri, Viewer};
use iced::widget::{button, checkbox, column, container, hover, row, scrollable, stack, text};
use iced::{Element, Fill, Font, alignment};

use super::Message;

/// The parser options iced's Markdown widget uses, so task markers found
/// here are the ones the preview draws.
const OPTIONS: pulldown_cmark::Options = pulldown_cmark::Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
    .union(pulldown_cmark::Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS)
    .union(pulldown_cmark::Options::ENABLE_TABLES)
    .union(pulldown_cmark::Options::ENABLE_STRIKETHROUGH)
    .union(pulldown_cmark::Options::ENABLE_TASKLISTS);

/// The text shown in the editor for a note: its H2 heading, then the body.
pub fn to_editor_text(title: &str, body: &str) -> String {
    if body.is_empty() {
        format!("## {title}\n\n")
    } else {
        format!("## {title}\n\n{body}")
    }
}

/// Splits editor text back into a title and body. The first non-blank line
/// is the title, with any leading `#`s removed; the rest is the body.
/// Returns `None` when the text is blank.
pub fn from_editor_text(text: &str) -> Option<(String, String)> {
    let text = text.trim_start_matches(['\n', '\r']);
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let title = first.trim().trim_start_matches('#').trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let body = rest.trim_start_matches(['\n', '\r']).trim_end();
    Some((title.to_string(), body.to_string()))
}

/// Ticks or unticks the `index`th task list item in `source`, counting in
/// document order. Returns the new source, or `None` if there is no such
/// task.
pub fn toggle_task(source: &str, index: usize, done: bool) -> Option<String> {
    let range = task_markers(source).nth(index)?;
    // The marker is `[ ]`, `[x]` or `[X]`; replace the character inside.
    let open = range.start + source[range].find('[')?;
    let inner = open + 1..open + 2;
    if !source.is_char_boundary(inner.end) {
        return None;
    }
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..inner.start]);
    out.push(if done { 'x' } else { ' ' });
    out.push_str(&source[inner.end..]);
    Some(out)
}

fn task_markers(source: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    pulldown_cmark::Parser::new_ext(source, OPTIONS)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, pulldown_cmark::Event::TaskListMarker(_)).then_some(range)
        })
}

/// The raw text of every code block in `items`, in the order [`NoteViewer`]
/// numbers them.
pub fn code_blocks(items: &[Item]) -> Vec<String> {
    fn walk(items: &[Item], out: &mut Vec<String>) {
        for item in items {
            match item {
                Item::CodeBlock { code, .. } => out.push(code.clone()),
                Item::List { bullets, .. } => {
                    for bullet in bullets {
                        walk(bullet_items(bullet), out);
                    }
                }
                Item::Quote(items) => walk(items, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(items, &mut out);
    out
}

/// The text a code block's Copy and Send buttons hand over: the code
/// without its fences or the newline that ends it.
pub fn block_text(code: &str) -> String {
    code.trim_end_matches(['\n', '\r']).to_string()
}

/// Where a Copy or Send button was pressed, so it can say "Copied".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// A code block: its place within the Markdown it belongs to.
    Block { target: TaskTarget, index: usize },
    /// A whole note's Copy button.
    Note { group: usize, note: usize },
    /// The Copy button of a GNOME extension setup step.
    Setup(usize),
}

/// What a button briefly says it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Copied,
    Sent,
}

/// Which Markdown a task checkbox belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTarget {
    /// A saved note, by its place in the panel.
    Note { group: usize, note: usize },
    /// The live preview of the note being edited.
    Editor,
}

/// Draws one note's body.
pub struct NoteViewer {
    target: TaskTarget,
    /// Number of the first code block in this note, counting from 0 across
    /// the whole panel.
    first_code_block: usize,
    /// A Zellij session is there to send code blocks to.
    can_send: bool,
    /// The block (by its index in this note) whose button just did
    /// something.
    flash: Option<(usize, Done)>,
    tasks: Cell<usize>,
    code_blocks: Cell<usize>,
}

impl NoteViewer {
    pub fn new(target: TaskTarget, first_code_block: usize) -> Self {
        Self {
            target,
            first_code_block,
            can_send: false,
            flash: None,
            tasks: Cell::new(0),
            code_blocks: Cell::new(0),
        }
    }

    /// Shows Send to Zellij buttons, and the button that just copied or
    /// sent (`flash`, from the panel) saying so.
    pub fn buttons(mut self, can_send: bool, flash: Option<(Spot, Done)>) -> Self {
        self.can_send = can_send;
        self.flash = match flash {
            Some((Spot::Block { target, index }, done)) if target == self.target => {
                Some((index, done))
            }
            _ => None,
        };
        self
    }

    /// Renders `items`. Use a fresh viewer for each render: it counts tasks
    /// and code blocks as it goes.
    pub fn view<'a>(&self, items: &'a [Item], settings: Settings) -> Element<'a, Message>
    where
        Self: 'a,
    {
        markdown::view_with(items, settings, self)
    }

    fn task(&self, done: bool, settings: Settings) -> Element<'static, Message> {
        let index = self.tasks.get();
        self.tasks.set(index + 1);
        let target = self.target;
        container(
            checkbox(done)
                .size(settings.text_size)
                .on_toggle(move |done| Message::ToggleTask {
                    target,
                    task: index,
                    done,
                }),
        )
        .center_y(text::LineHeight::default().to_absolute(settings.text_size))
        .into()
    }

    fn bullets<'a>(
        &self,
        settings: Settings,
        bullets: &'a [Bullet],
        marker: impl Fn(usize) -> Element<'a, Message>,
    ) -> Element<'a, Message> {
        let nested = Settings {
            spacing: settings.spacing * 0.6,
            ..settings
        };
        column(bullets.iter().enumerate().map(|(i, bullet)| {
            // Number the task before its nested items, matching source order.
            let marker = match bullet {
                Bullet::Task { done, .. } => self.task(*done, settings),
                Bullet::Point { .. } => marker(i),
            };
            row![
                marker,
                markdown::view_with(bullet_items(bullet), nested, self)
            ]
            .spacing(settings.spacing)
            .into()
        }))
        .spacing(settings.spacing * 0.75)
        .padding([0.0, settings.spacing.0])
        .into()
    }
}

/// A code block as plain monospace text. iced's own code block styles its
/// lines with the body font when no highlighter is enabled.
fn code_block<'a>(settings: Settings, code: &'a str) -> Element<'a, Message> {
    let scrollbar = scrollable::Scrollbar::default()
        .width(settings.code_size / 2)
        .scroller_width(settings.code_size / 2);
    container(
        scrollable(
            container(
                text(code.trim_end_matches('\n'))
                    .font(Font::MONOSPACE)
                    .size(settings.code_size),
            )
            .padding(settings.code_size),
        )
        .direction(scrollable::Direction::Horizontal(scrollbar)),
    )
    .width(Fill)
    .padding(settings.code_size / 4)
    .style(container::dark)
    .into()
}

/// The Copy and Send to Zellij buttons, over a code block's top right.
fn block_buttons<'a>(
    code: &str,
    spot: Spot,
    key: Option<usize>,
    can_send: bool,
    flash: Option<Done>,
) -> Element<'a, Message> {
    let small = |label: &'static str| text(label).size(11);
    let copy = button(small(if flash == Some(Done::Copied) {
        "Copied"
    } else {
        "Copy"
    }))
    .padding([2, 6])
    .style(button::secondary)
    .on_press(Message::Copy {
        text: code.to_string(),
        spot,
    });
    let copy_tip = match key {
        Some(n) => format!("Copy without the fences ({n})"),
        None => "Copy without the fences".into(),
    };
    let mut bar = row![super::hint(copy, copy_tip)].spacing(4);
    if can_send {
        let send = button(small(if flash == Some(Done::Sent) {
            "Sent"
        } else {
            "Send to Zellij"
        }))
        .padding([2, 6])
        .style(button::secondary)
        .on_press(Message::SendToZellij {
            text: code.to_string(),
            spot,
        });
        let send_tip = match key {
            Some(n) => {
                format!("Type it into the last Zellij pane (Shift+{n}). Shift+click runs it.")
            }
            None => "Type it into the last Zellij pane. Shift+click runs it.".into(),
        };
        bar = bar.push(super::hint(send, send_tip));
    }
    container(bar).align_right(Fill).padding(4).into()
}

fn bullet_items(bullet: &Bullet) -> &[Item] {
    match bullet {
        Bullet::Point { items } | Bullet::Task { items, .. } => items,
    }
}

impl<'a> Viewer<'a, Message> for NoteViewer {
    fn on_link_click(url: Uri) -> Message {
        Message::LinkClicked(url)
    }

    fn unordered_list(&self, settings: Settings, bullets: &'a [Bullet]) -> Element<'a, Message> {
        self.bullets(settings, bullets, |_| {
            text("•").size(settings.text_size).into()
        })
    }

    fn ordered_list(
        &self,
        settings: Settings,
        start: u64,
        bullets: &'a [Bullet],
    ) -> Element<'a, Message> {
        self.bullets(settings, bullets, move |i| {
            text!("{}.", start + i as u64)
                .size(settings.text_size)
                .align_x(alignment::Horizontal::Right)
                .into()
        })
    }

    fn code_block(
        &self,
        settings: Settings,
        _language: Option<&'a str>,
        code: &'a str,
        _lines: &'a [markdown::Text],
    ) -> Element<'a, Message> {
        let index = self.code_blocks.get();
        self.code_blocks.set(index + 1);
        // Keys 1 to 9 reach the first nine blocks of the saved notes; the
        // preview of a note being edited isn't numbered.
        let key = match self.target {
            TaskTarget::Note { .. } => Some(self.first_code_block + index + 1).filter(|n| *n <= 9),
            TaskTarget::Editor => None,
        };
        let spot = Spot::Block {
            target: self.target,
            index,
        };
        let flash = self
            .flash
            .and_then(|(i, done)| (i == index).then_some(done));
        let block = code_block(settings, code);
        let buttons = block_buttons(&block_text(code), spot, key, self.can_send, flash);
        if flash.is_some() {
            // Keep "Copied" in view when a number key did it.
            stack![block, buttons].into()
        } else {
            hover(block, buttons)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_text_round_trips() {
        let text = to_editor_text("Dev setup", "Run `cargo run`.");
        assert_eq!(text, "## Dev setup\n\nRun `cargo run`.");
        assert_eq!(
            from_editor_text(&text),
            Some(("Dev setup".into(), "Run `cargo run`.".into()))
        );
        assert_eq!(
            from_editor_text("\n  Plain first line\nbody\n\n"),
            Some(("Plain first line".into(), "body".into()))
        );
        assert_eq!(
            from_editor_text("## \nbody"),
            Some(("Untitled".into(), "body".into()))
        );
        assert_eq!(from_editor_text(" \n\n "), None);
        assert_eq!(
            from_editor_text(&to_editor_text("Empty", "")),
            Some(("Empty".into(), String::new()))
        );
    }

    #[test]
    fn toggles_tasks_in_document_order() {
        let src =
            "- [ ] one\n  - [x] nested\n- plain\n\n1. [X] numbered\n\n```\n- [ ] not a task\n```\n";
        let t0 = toggle_task(src, 0, true).unwrap();
        assert!(t0.starts_with("- [x] one\n"));
        let t1 = toggle_task(src, 1, false).unwrap();
        assert!(t1.contains("  - [ ] nested\n"));
        let t2 = toggle_task(src, 2, false).unwrap();
        assert!(t2.contains("1. [ ] numbered"));
        assert_eq!(toggle_task(src, 3, true), None);
        // Only the marker changes.
        assert_eq!(t0.len(), src.len());
    }

    #[test]
    fn viewer_numbers_tasks_like_the_source() {
        // The preview's checkboxes must line up with toggle_task's indices.
        let src = "- [ ] a\n  - [x] b\n- c\n  - [ ] d\n\n> - [ ] e\n\n1. [ ] f\n";
        let content = markdown::Content::parse(src);
        let viewer = NoteViewer::new(TaskTarget::Editor, 0);
        let _ = viewer.view(
            content.items(),
            Settings::with_text_size(14, iced::Theme::Light),
        );
        assert_eq!(viewer.tasks.get(), task_markers(src).count());
        assert_eq!(viewer.tasks.get(), 5);
    }

    #[test]
    fn lists_code_blocks_in_order() {
        let src =
            "```sh\ncargo run\n```\n\n- item\n\n  ```\nnested\n  ```\n\n> ```\n> quoted\n> ```\n";
        let content = markdown::Content::parse(src);
        let blocks = code_blocks(content.items());
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].trim_end(), "cargo run");

        let viewer = NoteViewer::new(TaskTarget::Editor, 5);
        let _ = viewer.view(
            content.items(),
            Settings::with_text_size(14, iced::Theme::Light),
        );
        assert_eq!(viewer.code_blocks.get(), blocks.len());
    }
}
