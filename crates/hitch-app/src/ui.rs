//! The panel, built with iced.
//!
//! Skeleton only: one narrow window with the empty state. The panel UI
//! (sections per context, pin, show/hide, quick capture, Markdown preview,
//! copy and send-to-Zellij buttons) is built on top of this.

use iced::widget::{center, text};
use iced::{Element, Size};

/// Panel width from the spec, about 320 px.
const PANEL_WIDTH: f32 = 320.0;

pub fn run() -> iced::Result {
    iced::application(Panel::default, Panel::update, Panel::view)
        .title("Hitch")
        .window_size(Size::new(PANEL_WIDTH, 480.0))
        .run()
}

#[derive(Debug, Default)]
struct Panel;

#[derive(Debug, Clone)]
enum Message {}

impl Panel {
    fn update(&mut self, message: Message) {
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        center(text("No notes here yet. Press Ctrl+Shift+H to add one.")).into()
    }
}
