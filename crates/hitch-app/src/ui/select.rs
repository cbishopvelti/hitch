//! Text that can be selected with the mouse and copied, as in any other
//! app. iced's own text widgets draw text but can't select it.
//!
//! [`Selectable`] draws rich text the way iced's `rich_text` does (links,
//! underline, strikethrough and highlighted spans like inline code), and
//! adds a selection:
//!
//! - drag to select, Shift+click to extend, double-click selects a word and
//!   triple-click a whole line;
//! - Ctrl+C (Cmd+C on macOS) copies the selection, and Ctrl+A selects all of
//!   the text the selection is in;
//! - a click anywhere else clears it.
//!
//! Each widget holds its own selection, so a selection stays within one
//! paragraph, heading or code block. A note's Copy button copies the whole
//! note.

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::graphics::text::cosmic_text::{self, Buffer};
use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{self as core_text, Paragraph as _, Span};
use iced::advanced::widget::text as text_widget;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, renderer};
use iced::keyboard::{self, Key};
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{
    Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme, Vector, alignment,
};
use unicode_segmentation::UnicodeSegmentation;

/// Selectable rich text. See the module docs.
pub struct Selectable<'a, Link, Message> {
    spans: Box<dyn AsRef<[Span<'a, Link>]> + 'a>,
    size: Option<Pixels>,
    line_height: LineHeight,
    font: Option<Font>,
    wrapping: Wrapping,
    color: Option<Color>,
    on_link_click: Option<Box<dyn Fn(Link) -> Message + 'a>>,
    on_copy: Box<dyn Fn(String) -> Message + 'a>,
}

/// Selectable rich text made of `spans`. `on_copy` is published with the
/// selected text when the user copies it.
pub fn rich<'a, Link, Message>(
    spans: impl AsRef<[Span<'a, Link>]> + 'a,
    on_copy: impl Fn(String) -> Message + 'a,
) -> Selectable<'a, Link, Message> {
    Selectable {
        spans: Box::new(spans),
        size: None,
        line_height: LineHeight::default(),
        font: None,
        wrapping: Wrapping::default(),
        color: None,
        on_link_click: None,
        on_copy: Box::new(on_copy),
    }
}

/// Selectable plain text.
pub fn plain<'a, Message>(
    text: impl Into<String>,
    on_copy: impl Fn(String) -> Message + 'a,
) -> Selectable<'a, (), Message> {
    rich(vec![Span::new(text.into())], on_copy)
}

impl<'a, Link, Message> Selectable<'a, Link, Message> {
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = Some(size.into());
        self
    }

    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    pub fn wrapping(mut self, wrapping: Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn on_link_click(mut self, on_link_click: impl Fn(Link) -> Message + 'a) -> Self {
        self.on_link_click = Some(Box::new(on_link_click));
        self
    }
}

/// A place in the text: a line of the paragraph (lines are split at `\n`,
/// not where the text wraps) and a byte offset within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub index: usize,
}

impl Pos {
    fn cursor(self) -> cosmic_text::Cursor {
        cosmic_text::Cursor::new(self.line, self.index)
    }
}

/// What a drag extends the selection by, and the part the press selected.
#[derive(Debug, Clone, Copy)]
enum Drag {
    Chars { anchor: Pos },
    Words { from: Pos, to: Pos },
    Lines { from: Pos, to: Pos },
}

struct State<Link> {
    spans: Vec<Span<'static, Link>>,
    paragraph: Paragraph,
    /// Where the selection started, and where it ends now.
    selection: Option<(Pos, Pos)>,
    drag: Option<Drag>,
    last_click: Option<mouse::Click>,
    link_pressed: Option<usize>,
    hovered_link: Option<usize>,
}

impl<Link> State<Link> {
    /// The selection in reading order, when it isn't empty.
    fn range(&self) -> Option<(Pos, Pos)> {
        let (a, b) = self.selection?;
        (a != b).then(|| if a < b { (a, b) } else { (b, a) })
    }
}

impl<Link, Message> Widget<Message, Theme, iced::Renderer> for Selectable<'_, Link, Message>
where
    Link: Clone + PartialEq + 'static,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Link>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Link> {
            spans: Vec::new(),
            paragraph: Paragraph::default(),
            selection: None,
            drag: None,
            last_click: None,
            link_pressed: None,
            hovered_link: None,
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        use iced::advanced::text::Renderer as _;

        let state = tree.state.downcast_mut::<State<Link>>();
        let spans = self.spans.as_ref().as_ref();
        layout::sized(limits, Length::Shrink, Length::Shrink, |limits| {
            let bounds = limits.max();
            let size = self.size.unwrap_or_else(|| renderer.default_size());
            let font = self.font.unwrap_or_else(|| renderer.default_font());
            let text = |content| core_text::Text {
                content,
                bounds,
                size,
                line_height: self.line_height,
                font,
                align_x: core_text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: Shaping::Advanced,
                wrapping: self.wrapping,
            };
            let shape = core_text::Text {
                content: (),
                bounds,
                size,
                line_height: self.line_height,
                font,
                align_x: core_text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: Shaping::Advanced,
                wrapping: self.wrapping,
            };

            if state.spans != spans {
                state.paragraph = Paragraph::with_spans(text(spans));
                state.spans = spans.iter().cloned().map(Span::to_static).collect();
                // The text changed under the selection.
                state.selection = None;
                state.drag = None;
            } else {
                match state.paragraph.compare(shape) {
                    core_text::Difference::None => {}
                    core_text::Difference::Bounds => state.paragraph.resize(bounds),
                    core_text::Difference::Shape => {
                        state.paragraph = Paragraph::with_spans(text(spans));
                    }
                }
            }
            state.paragraph.min_bounds()
        })
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::text::Renderer as _;

        let bounds = layout.bounds();
        if !bounds.intersects(viewport) {
            return;
        }
        let state = tree.state.downcast_ref::<State<Link>>();
        let translation = bounds.position() - Point::ORIGIN;
        let size = self.size.unwrap_or_else(|| renderer.default_size());

        let text_color = self.color.unwrap_or(defaults.text_color);
        for (index, span) in self.spans.as_ref().as_ref().iter().enumerate() {
            let hovered_link = self.on_link_click.is_some() && Some(index) == state.hovered_link;
            if span.highlight.is_none() && !span.underline && !span.strikethrough && !hovered_link {
                continue;
            }
            let regions = state.paragraph.span_bounds(index);
            if let Some(highlight) = span.highlight {
                for region in &regions {
                    let region = Rectangle::new(
                        region.position() - Vector::new(span.padding.left, span.padding.top),
                        region.size() + Size::new(span.padding.x(), span.padding.y()),
                    );
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: region + translation,
                            border: highlight.border,
                            ..Default::default()
                        },
                        highlight.background,
                    );
                }
            }
            if span.underline || span.strikethrough || hovered_link {
                let span_size = span.size.or(self.size).unwrap_or(size);
                let line_height = span
                    .line_height
                    .unwrap_or(self.line_height)
                    .to_absolute(span_size);
                let color = span.color.unwrap_or(text_color);
                let baseline = translation
                    + Vector::new(0.0, span_size.0 + (line_height.0 - span_size.0) / 2.0);
                let mut line = |offset: f32| {
                    for region in &regions {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle::new(
                                    region.position() + baseline - Vector::new(0.0, offset),
                                    Size::new(region.width, 1.0),
                                ),
                                ..Default::default()
                            },
                            color,
                        );
                    }
                };
                if span.underline || hovered_link {
                    line(span_size.0 * 0.08);
                }
                if span.strikethrough {
                    line(span_size.0 / 2.0);
                }
            }
        }

        if let Some((start, end)) = state.range() {
            // See-through, so inline code and underlines show under it.
            let color = Color {
                a: 0.6,
                ..theme.extended_palette().primary.weak.color
            };
            for rect in selection_rects(state.paragraph.buffer(), start, end, size.0) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: rect + translation,
                        ..Default::default()
                    },
                    color,
                );
            }
        }

        text_widget::draw(
            renderer,
            defaults,
            bounds,
            &state.paragraph,
            text_widget::Style { color: self.color },
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State<Link>>();
        let bounds = layout.bounds();
        let spans = self.spans.as_ref().as_ref();

        let hovered = match (&self.on_link_click, cursor.position_in(bounds)) {
            (Some(_), Some(position)) if state.drag.is_none() => state
                .paragraph
                .hit_span(position)
                .filter(|&span| spans.get(span).is_some_and(|span| span.link.is_some())),
            _ => None,
        };
        if hovered != state.hovered_link {
            state.hovered_link = hovered;
            shell.request_redraw();
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_in(bounds) else {
                    // A click anywhere else ends the selection.
                    if state.selection.take().is_some() {
                        shell.request_redraw();
                    }
                    state.drag = None;
                    return;
                };
                if state.hovered_link.is_some() {
                    state.link_pressed = state.hovered_link;
                    state.selection = None;
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
                let buffer = state.paragraph.buffer();
                let pos = hit(buffer, bounds.size(), position);
                let click = mouse::Click::new(position, mouse::Button::Left, state.last_click);
                state.last_click = Some(click);
                let (selection, drag) = match click.kind() {
                    mouse::click::Kind::Single => {
                        let anchor = match state.selection {
                            Some((anchor, _)) if shift_held() => anchor,
                            _ => pos,
                        };
                        ((anchor, pos), Drag::Chars { anchor })
                    }
                    mouse::click::Kind::Double => {
                        let (from, to) = word_at(buffer, pos);
                        ((from, to), Drag::Words { from, to })
                    }
                    mouse::click::Kind::Triple => {
                        let (from, to) = line_at(buffer, pos);
                        ((from, to), Drag::Lines { from, to })
                    }
                };
                state.selection = Some(selection);
                state.drag = Some(drag);
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let (Some(drag), Some(position)) = (state.drag, cursor.position()) else {
                    return;
                };
                let buffer = state.paragraph.buffer();
                let position = position - (bounds.position() - Point::ORIGIN);
                let pos = hit(buffer, bounds.size(), position);
                let selection = match drag {
                    Drag::Chars { anchor } => (anchor, pos),
                    Drag::Words { from, to } => extend((from, to), word_at(buffer, pos)),
                    Drag::Lines { from, to } => extend((from, to), line_at(buffer, pos)),
                };
                if state.selection != Some(selection) {
                    state.selection = Some(selection);
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag = None;
                if let Some(pressed) = state.link_pressed.take()
                    && Some(pressed) == state.hovered_link
                    && let Some(on_link_click) = &self.on_link_click
                    && let Some(link) = spans.get(pressed).and_then(|span| span.link.clone())
                {
                    shell.publish(on_link_click(link));
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if modifiers.command() && !modifiers.alt() =>
            {
                let Key::Character(c) = key.as_ref() else {
                    return;
                };
                match c.to_ascii_lowercase().as_str() {
                    "c" => {
                        if let Some((start, end)) = state.range() {
                            let text = text_between(state.paragraph.buffer(), start, end);
                            shell.publish((self.on_copy)(text));
                            shell.capture_event();
                        }
                    }
                    "a" if state.selection.is_some() => {
                        let buffer = state.paragraph.buffer();
                        state.selection = Some((Pos::default(), end_of(buffer)));
                        shell.capture_event();
                        shell.request_redraw();
                    }
                    _ => {}
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                SHIFT.with(|shift| shift.set(modifiers.shift()));
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State<Link>>();
        if state.hovered_link.is_some() {
            mouse::Interaction::Pointer
        } else if state.drag.is_some() || cursor.is_over(layout.bounds()) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }
}

thread_local! {
    /// Whether Shift is held, for Shift+click. Mouse events don't carry the
    /// modifiers, and every selectable text sees the same keyboard.
    static SHIFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn shift_held() -> bool {
    SHIFT.with(std::cell::Cell::get)
}

impl<'a, Link, Message> From<Selectable<'a, Link, Message>> for Element<'a, Message>
where
    Link: Clone + PartialEq + 'static,
    Message: 'a,
{
    fn from(text: Selectable<'a, Link, Message>) -> Self {
        Element::new(text)
    }
}

/// The place in the text at `point`, relative to the text's top left.
/// Above the text is its start and below it its end.
fn hit(buffer: &Buffer, size: Size, point: Point) -> Pos {
    if point.y < 0.0 {
        return Pos::default();
    }
    if point.y >= size.height {
        return end_of(buffer);
    }
    buffer
        .hit(point.x.max(0.0), point.y)
        .map(|cursor| Pos {
            line: cursor.line,
            index: cursor.index,
        })
        .unwrap_or_else(|| end_of(buffer))
}

fn end_of(buffer: &Buffer) -> Pos {
    let line = buffer.lines.len().saturating_sub(1);
    let index = buffer.lines.get(line).map_or(0, |l| l.text().len());
    Pos { line, index }
}

/// The text from `start` to `end`, with `\n` between lines.
pub fn text_between(buffer: &Buffer, start: Pos, end: Pos) -> String {
    let lines: Vec<&str> = buffer.lines.iter().map(|line| line.text()).collect();
    slice_lines(&lines, start, end)
}

fn slice_lines(lines: &[&str], start: Pos, end: Pos) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().take(end.line + 1).skip(start.line) {
        let from = if i == start.line { start.index } else { 0 };
        let to = if i == end.line { end.index } else { line.len() };
        out.push_str(line.get(from.min(to)..to).unwrap_or_default());
        if i != end.line {
            out.push('\n');
        }
    }
    out
}

/// The word (or run of spaces or punctuation) at `pos`.
fn word_at(buffer: &Buffer, pos: Pos) -> (Pos, Pos) {
    let text = buffer.lines.get(pos.line).map_or("", |l| l.text());
    let (from, to) = word_bounds(text, pos.index);
    (
        Pos {
            line: pos.line,
            index: from,
        },
        Pos {
            line: pos.line,
            index: to,
        },
    )
}

fn word_bounds(text: &str, index: usize) -> (usize, usize) {
    let mut last = (text.len(), text.len());
    for (start, word) in text.split_word_bound_indices() {
        let end = start + word.len();
        last = (start, end);
        if index < end {
            return last;
        }
    }
    last
}

fn line_at(buffer: &Buffer, pos: Pos) -> (Pos, Pos) {
    let len = buffer.lines.get(pos.line).map_or(0, |l| l.text().len());
    (
        Pos {
            line: pos.line,
            index: 0,
        },
        Pos {
            line: pos.line,
            index: len,
        },
    )
}

/// The selection a word or line drag makes: from the part first selected
/// to the part under the pointer, whichever way the drag went.
fn extend(first: (Pos, Pos), now: (Pos, Pos)) -> (Pos, Pos) {
    if now.0 < first.0 {
        (first.1, now.0)
    } else {
        (first.0, first.1.max(now.1))
    }
}

/// The rectangles to shade for the selection from `start` to `end`.
fn selection_rects(buffer: &Buffer, start: Pos, end: Pos, size: f32) -> Vec<Rectangle> {
    let (from, to) = (start.cursor(), end.cursor());
    buffer
        .layout_runs()
        .filter_map(|run| {
            let (x, width) = match run.highlight(from, to) {
                Some((x, width)) if width > 0.0 => (x, width),
                // An empty line inside the selection still shows it.
                _ if run.glyphs.is_empty() && run.line_i >= start.line && run.line_i < end.line => {
                    (0.0, size / 3.0)
                }
                _ => return None,
            };
            Some(Rectangle::new(
                Point::new(x, run.line_top),
                Size::new(width, run.line_height),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, index: usize) -> Pos {
        Pos { line, index }
    }

    #[test]
    fn slices_across_lines() {
        let lines = ["cargo build", "", "cargo run --release"];
        assert_eq!(slice_lines(&lines, pos(0, 6), pos(0, 11)), "build");
        assert_eq!(
            slice_lines(&lines, pos(0, 6), pos(2, 9)),
            "build\n\ncargo run"
        );
        assert_eq!(
            slice_lines(&lines, pos(0, 0), pos(2, 19)),
            "cargo build\n\ncargo run --release"
        );
        // Offsets past a line's end don't panic.
        assert_eq!(slice_lines(&lines, pos(0, 40), pos(0, 50)), "");
    }

    #[test]
    fn finds_words() {
        let text = "run cargo-build now";
        assert_eq!(word_bounds(text, 0), (0, 3));
        assert_eq!(word_bounds(text, 5), (4, 9));
        assert_eq!(word_bounds(text, 3), (3, 4));
        assert_eq!(word_bounds(text, 19), (16, 19));
        assert_eq!(word_bounds("", 0), (0, 0));
    }

    #[test]
    fn word_drag_keeps_the_first_word() {
        let first = (pos(0, 4), pos(0, 9));
        assert_eq!(
            extend(first, (pos(0, 10), pos(0, 13))),
            (pos(0, 4), pos(0, 13))
        );
        assert_eq!(
            extend(first, (pos(0, 0), pos(0, 3))),
            (pos(0, 9), pos(0, 0))
        );
        assert_eq!(extend(first, first), first);
    }
}
