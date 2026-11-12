//! The panel, built with iced.
//!
//! Hitch is one narrow floating panel (spec, UX). It lists the notes for the
//! active contexts, one section per notes file, most specific first, renders
//! them as Markdown and edits them in place.
//!
//! - **Show and hide**: the panel is a window Hitch opens and closes; the
//!   app keeps running with no window (an iced daemon). `hitch toggle`, or
//!   Ctrl+Shift+N while the panel has focus, flips it. Closing the window
//!   only hides it; Ctrl+Q quits.
//! - **Pin**: keeps the panel above other windows until unpinned, and is
//!   remembered across restarts. Unpinned, the panel hides itself when it
//!   loses focus. On X11 and macOS the window level does this; on Wayland
//!   only the shell can, so the context engine asks the GNOME extension
//!   (`SetKeepAbove`, see docs/PROTOCOL.md).
//! - **Notes**: each note is an H2 section. "Edit" opens it as Markdown with
//!   a live preview; Esc or Ctrl+S saves. "+ Note" (or `hitch capture`,
//!   Ctrl+Shift+H) starts a new note hitched to the most specific context,
//!   with a dropdown to pick another. Task checkboxes toggle in place.

mod backend;
mod label;
mod note_view;
mod panel_state;

use std::fmt;
use std::path::PathBuf;

use hitch_context::ActiveContexts;
use hitch_protocol::{Command, Context};
use hitch_store::Note;
use iced::keyboard::{self, Key, Modifiers, key};
use iced::widget::markdown::{self, Uri};
use iced::widget::{
    button, center, column, container, mouse_area, pick_list, row, rule, scrollable, space, text,
    text_editor, tooltip,
};
use iced::window::{self, Level};
use iced::{
    Element, Event, Fill, Font, Point, Size, Subscription, Task, Theme, event, font, system, theme,
};

use backend::{Engine, NoteGroup, Store};
use note_view::{NoteViewer, TaskTarget};
use panel_state::{Edge, PanelState};

/// Panel size from the spec: about 320 px wide.
const PANEL_WIDTH: f32 = 320.0;
const PANEL_HEIGHT: f32 = 560.0;
/// Gap between the docked panel and the screen edge (and the top bar).
const DOCK_MARGIN: f32 = 48.0;
const TEXT_SIZE: u32 = 14;

#[cfg(target_os = "macos")]
const MOD: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "Ctrl";

const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

/// Starts Hitch: the context engine, then the panel. If Hitch is already
/// running, shows its panel instead.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let engine = match Engine::start() {
        Ok(engine) => engine,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            crate::cli::execute(crate::cli::Invocation::Command(Command::ShowPanel))?;
            return Ok(());
        }
        Err(e) => return Err(format!("can't start the context engine: {e}").into()),
    };
    // iced asks for a boot function it may call more than once; the engine
    // is handed over on the first call.
    let engine = std::cell::Cell::new(Some(engine));
    let boot = move || Panel::boot(engine.take().expect("Hitch boots once"));
    iced::daemon(boot, Panel::update, Panel::view)
        .title(Panel::title)
        .theme(Panel::theme)
        .subscription(Panel::subscription)
        .run()?;
    Ok(())
}

struct Panel {
    /// The panel window, when shown.
    window: Option<PanelWindow>,
    state: PanelState,
    state_path: Option<PathBuf>,
    engine: Engine,
    store: Store,
    contexts: ActiveContexts,
    groups: Vec<Group>,
    /// Where a new note can be hitched, most specific first.
    choices: Vec<Choice>,
    editor: Option<Editor>,
    mode: theme::Mode,
    /// The last thing that went wrong, shown under the notes until the next
    /// action succeeds.
    error: Option<String>,
    /// Setup hints from the engine, until dismissed.
    notices: Vec<Notice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The GNOME Shell extension isn't on the bus (Wayland).
    NoShellExtension,
    /// Zed's window title has no file path.
    ZedTitleSetting,
}

impl Notice {
    fn text(self) -> String {
        match self {
            Notice::NoShellExtension => "Hitch can't see which window has focus or keep the \
                panel on top: the Hitch GNOME Shell extension isn't running."
                .into(),
            Notice::ZedTitleSetting => format!(
                "Zed's window title doesn't show the file, so Hitch can't tell which file \
                 is open. Add {} to ~/.config/zed/settings.json.",
                hitch_protocol::zed::RECOMMENDED_SETTINGS
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PanelWindow {
    id: window::Id,
    /// Whether it has had focus since it opened. Auto-hide waits for this,
    /// so a panel the compositor opened without focus doesn't vanish.
    focused: bool,
}

/// One section of the panel: the notes of one notes file.
struct Group {
    path: PathBuf,
    label: String,
    kind: &'static str,
    notes: Vec<ShownNote>,
}

struct ShownNote {
    note: Note,
    preview: markdown::Content,
    /// Panel-wide number of this note's first code block.
    first_code_block: usize,
}

/// A place a new note can be hitched to.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    context: Context,
    path: PathBuf,
    label: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

struct Editor {
    target: Target,
    content: text_editor::Content,
    preview: markdown::Content,
    /// "Delete" was pressed once; the next press deletes.
    confirm_delete: bool,
}

enum Target {
    Existing { path: PathBuf, note: Note },
    New { choice: Choice },
}

#[derive(Debug, Clone)]
pub enum Message {
    Engine(backend::Event),
    Dismiss(Notice),
    Window(window::Id, WindowEvent),
    Shortcut(Shortcut),
    ThemeChanged(theme::Mode),
    Drag,
    Hide,
    TogglePin,
    NewNote,
    Edit {
        group: usize,
        note: usize,
    },
    HitchTo(Choice),
    EditorAction(text_editor::Action),
    SaveEdit,
    DiscardEdit,
    DeleteNote,
    ToggleTask {
        target: TaskTarget,
        task: usize,
        done: bool,
    },
    LinkClicked(Uri),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    Focused,
    Unfocused,
    CloseRequested,
    Closed,
}

/// Keys handled while the panel has focus. The global shortcuts reach a
/// hidden panel through `hitch toggle` and friends instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Save the note being edited; with no editor, hide an unpinned panel.
    Escape,
    /// Ctrl/Cmd+S.
    Save,
    /// Ctrl/Cmd+Shift+N.
    TogglePanel,
    /// Ctrl/Cmd+Shift+H.
    Capture,
    /// Ctrl/Cmd+Shift+P.
    TogglePin,
    /// Ctrl/Cmd+Q.
    Quit,
}

impl Panel {
    fn boot(engine: Engine) -> (Self, Task<Message>) {
        let state_path = PanelState::default_path();
        let state = state_path
            .as_deref()
            .map(PanelState::load)
            .unwrap_or_default();
        let mut panel = Self {
            window: None,
            state,
            state_path,
            contexts: engine.current(),
            engine,
            store: Store::for_user(),
            groups: Vec::new(),
            choices: Vec::new(),
            editor: None,
            mode: theme::Mode::None,
            error: None,
            notices: Vec::new(),
        };
        if panel.state.pinned {
            panel.keep_above();
        }
        let show = panel.show();
        (
            panel,
            Task::batch([show, system::theme().map(Message::ThemeChanged)]),
        )
    }

    fn title(&self, _window: window::Id) -> String {
        "Hitch".into()
    }

    fn theme(&self, _window: window::Id) -> Option<Theme> {
        match self.mode {
            theme::Mode::Dark => Some(Theme::Dark),
            theme::Mode::Light => Some(Theme::Light),
            theme::Mode::None => None,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            self.engine.subscription().map(Message::Engine),
            event::listen_with(on_event),
            system::theme_changes().map(Message::ThemeChanged),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Engine(backend::Event::Contexts(contexts)) => {
                if contexts != self.contexts {
                    self.contexts = contexts;
                    self.reload();
                }
                Task::none()
            }
            Message::Engine(backend::Event::Command(command)) => self.command(command),
            Message::Engine(backend::Event::ShellExtension { available }) => {
                // Only GNOME on Wayland needs the extension.
                if available || !is_wayland() {
                    self.notices.retain(|n| *n != Notice::NoShellExtension);
                } else {
                    self.notice(Notice::NoShellExtension);
                }
                Task::none()
            }
            Message::Engine(backend::Event::ZedNeedsTitleSetting) => {
                self.notice(Notice::ZedTitleSetting);
                Task::none()
            }
            Message::Engine(_) => Task::none(),
            Message::Dismiss(notice) => {
                self.notices.retain(|n| *n != notice);
                Task::none()
            }
            Message::Window(id, event) => self.window_event(id, event),
            Message::Shortcut(shortcut) => self.shortcut(shortcut),
            Message::ThemeChanged(mode) => {
                self.mode = mode;
                Task::none()
            }
            Message::Drag => match self.window {
                Some(w) => window::drag(w.id),
                None => Task::none(),
            },
            Message::Hide => self.hide(),
            Message::TogglePin => self.toggle_pin(),
            Message::NewNote => self.start_new_note(),
            Message::Edit { group, note } => self.start_edit(group, note),
            Message::HitchTo(choice) => {
                if let Some(Editor {
                    target: Target::New { choice: current },
                    ..
                }) = &mut self.editor
                {
                    *current = choice;
                }
                Task::none()
            }
            Message::EditorAction(action) => {
                if let Some(editor) = &mut self.editor {
                    let edit = action.is_edit();
                    editor.content.perform(action);
                    if edit {
                        editor.preview = markdown::Content::parse(&editor.content.text());
                        editor.confirm_delete = false;
                    }
                }
                Task::none()
            }
            Message::SaveEdit => {
                self.commit_edit();
                Task::none()
            }
            Message::DiscardEdit => {
                self.editor = None;
                Task::none()
            }
            Message::DeleteNote => {
                self.delete_edited_note();
                Task::none()
            }
            Message::ToggleTask { target, task, done } => {
                self.toggle_task(target, task, done);
                Task::none()
            }
            Message::LinkClicked(url) => {
                self.report(open_url(&url));
                Task::none()
            }
        }
    }

    fn command(&mut self, command: Command) -> Task<Message> {
        match command {
            Command::TogglePanel if self.window.is_some() => self.hide(),
            Command::TogglePanel | Command::ShowPanel => self.show(),
            Command::HidePanel => self.hide(),
            Command::TogglePin => self.toggle_pin(),
            Command::Capture => Task::batch([self.show(), self.start_new_note()]),
            // Search is its own task; until then it just shows the panel.
            Command::Search => self.show(),
        }
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        match shortcut {
            Shortcut::Escape if self.editor.is_some() => {
                self.commit_edit();
                Task::none()
            }
            Shortcut::Escape if !self.state.pinned => self.hide(),
            Shortcut::Escape => Task::none(),
            Shortcut::Save => {
                self.commit_edit();
                Task::none()
            }
            Shortcut::TogglePanel => self.command(Command::TogglePanel),
            Shortcut::Capture => self.command(Command::Capture),
            Shortcut::TogglePin => self.toggle_pin(),
            Shortcut::Quit => {
                self.commit_edit();
                iced::exit()
            }
        }
    }

    fn window_event(&mut self, id: window::Id, event: WindowEvent) -> Task<Message> {
        let Some(window) = self.window.as_mut().filter(|w| w.id == id) else {
            return Task::none();
        };
        match event {
            WindowEvent::Focused => {
                window.focused = true;
                Task::none()
            }
            // Unpinned, the panel auto-hides (spec, UX).
            WindowEvent::Unfocused if window.focused && !self.state.pinned => self.hide(),
            WindowEvent::Unfocused => Task::none(),
            WindowEvent::CloseRequested => self.hide(),
            WindowEvent::Closed => {
                self.window = None;
                Task::none()
            }
        }
    }

    /// Opens the panel, or raises it if it is already open. Notes are
    /// re-read so edits made outside Hitch show up.
    fn show(&mut self) -> Task<Message> {
        self.reload();
        if let Some(window) = self.window {
            return window::gain_focus(window.id);
        }
        let (id, open) = window::open(self.window_settings());
        self.window = Some(PanelWindow { id, focused: false });
        open.discard()
    }

    /// Closes the panel window, saving any note being edited. Hitch keeps
    /// running.
    fn hide(&mut self) -> Task<Message> {
        self.commit_edit();
        match self.window.take() {
            Some(window) => window::close(window.id),
            None => Task::none(),
        }
    }

    fn toggle_pin(&mut self) -> Task<Message> {
        self.state.pinned = !self.state.pinned;
        if let Some(path) = &self.state_path {
            self.report(self.state.save(path));
        }
        self.keep_above();
        match self.window {
            Some(window) => window::set_level(window.id, self.level()),
            None => Task::none(),
        }
    }

    /// Pins or unpins through the GNOME extension. Only Wayland needs it; the
    /// window level covers X11 and macOS.
    fn keep_above(&mut self) {
        if let Err(e) = self.engine.set_keep_above(self.state.pinned)
            && self.state.pinned
            && is_wayland()
        {
            self.error = Some(format!(
                "Pinned, but the panel can't stay on top until the Hitch GNOME extension runs ({e})."
            ));
        }
    }

    fn notice(&mut self, notice: Notice) {
        if !self.notices.contains(&notice) {
            self.notices.push(notice);
        }
    }

    fn level(&self) -> Level {
        if self.state.pinned {
            Level::AlwaysOnTop
        } else {
            Level::Normal
        }
    }

    fn window_settings(&self) -> window::Settings {
        window::Settings {
            size: Size::new(PANEL_WIDTH, PANEL_HEIGHT),
            min_size: Some(Size::new(240.0, 200.0)),
            // Compositors may ignore this (Wayland does).
            position: window::Position::SpecificWith(match self.state.edge {
                Edge::Left => dock_left,
                Edge::Right => dock_right,
            }),
            decorations: false,
            level: self.level(),
            // Closing the window hides the panel; it doesn't quit Hitch.
            exit_on_close_request: false,
            #[cfg(target_os = "linux")]
            platform_specific: window::settings::PlatformSpecific {
                application_id: hitch_protocol::APP_ID.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Re-reads the notes for the active contexts.
    fn reload(&mut self) {
        self.choices = self
            .contexts
            .contexts
            .iter()
            .map(|context| Choice {
                path: self.store.notes_path(context),
                label: match context {
                    Context::Folder { path } => label::tilde(path),
                    other => label::context(other),
                },
                context: context.clone(),
            })
            .collect();

        let groups = match self.store.notes_for_all(&self.contexts.contexts) {
            Ok(groups) => groups,
            Err(e) => {
                self.error = Some(format!("Couldn't read notes: {e}"));
                return;
            }
        };
        let mut code_blocks = 0;
        self.groups = groups
            .into_iter()
            .map(
                |NoteGroup {
                     path,
                     context,
                     notes,
                 }| Group {
                    path,
                    label: label::context(&context),
                    kind: label::kind(&context),
                    notes: notes
                        .into_iter()
                        .map(|note| {
                            let preview = markdown::Content::parse(&note.body);
                            let first_code_block = code_blocks;
                            code_blocks += note_view::code_blocks(preview.items()).len();
                            ShownNote {
                                note,
                                preview,
                                first_code_block,
                            }
                        })
                        .collect(),
                },
            )
            .collect();

        // A new note's target may have gone away with the old contexts.
        if let Some(Editor {
            target: Target::New { choice },
            ..
        }) = &mut self.editor
            && !self.choices.contains(choice)
            && let Some(first) = self.choices.first()
        {
            *choice = first.clone();
        }
    }

    fn start_new_note(&mut self) -> Task<Message> {
        self.commit_edit();
        let Some(choice) = self.choices.first().cloned() else {
            self.error = Some("Nothing to hitch a note to yet.".into());
            return Task::none();
        };
        self.open_editor(Target::New { choice }, "## ")
    }

    fn start_edit(&mut self, group: usize, note: usize) -> Task<Message> {
        self.commit_edit();
        let Some((path, shown)) = self.note(group, note) else {
            return Task::none();
        };
        let text = note_view::to_editor_text(&shown.note.title, &shown.note.body);
        let target = Target::Existing {
            path: path.clone(),
            note: shown.note.clone(),
        };
        self.open_editor(target, &text)
    }

    fn open_editor(&mut self, target: Target, text: &str) -> Task<Message> {
        let mut content = text_editor::Content::with_text(text);
        content.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        self.editor = Some(Editor {
            target,
            preview: markdown::Content::parse(text),
            content,
            confirm_delete: false,
        });
        iced::widget::operation::focus(EDITOR_ID)
    }

    /// Saves the note being edited, if any. On failure the editor stays open
    /// so nothing typed is lost.
    fn commit_edit(&mut self) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        let parsed = note_view::from_editor_text(&editor.content.text());
        let result = match (&editor.target, parsed) {
            // Blank: a new note is dropped, an existing one left as it was.
            (_, None) => Ok(()),
            (Target::Existing { path, note }, Some((title, body))) => {
                if note.title == title && note.body == body {
                    Ok(())
                } else {
                    let note = Note {
                        title,
                        body,
                        ..note.clone()
                    };
                    self.store.save_note(path, note).map(drop)
                }
            }
            (Target::New { choice }, Some((title, body))) => {
                // TODO(store): tag the note with the file for a File context
                // (spec, matching rule 4) once Note carries its files.
                let note = Note {
                    title,
                    body,
                    ..Default::default()
                };
                self.store.save_note(&choice.path, note).map(drop)
            }
        };
        match result {
            Ok(()) => {
                self.error = None;
                self.reload();
            }
            Err(e) => {
                self.error = Some(format!("Couldn't save the note: {e}"));
                self.editor = Some(editor);
            }
        }
    }

    fn delete_edited_note(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Target::Existing { path, note } = &editor.target else {
            // A new note isn't saved yet: deleting it is discarding it.
            self.editor = None;
            return;
        };
        if !editor.confirm_delete {
            editor.confirm_delete = true;
            return;
        }
        let Some(id) = note.id.clone() else {
            return;
        };
        let path = path.clone();
        match self.store.delete_note(&path, &id) {
            Ok(_) => {
                self.editor = None;
                self.error = None;
                self.reload();
            }
            Err(e) => self.error = Some(format!("Couldn't delete the note: {e}")),
        }
    }

    fn toggle_task(&mut self, target: TaskTarget, task: usize, done: bool) {
        match target {
            TaskTarget::Editor => {
                if let Some(editor) = &mut self.editor
                    && let Some(text) = note_view::toggle_task(&editor.content.text(), task, done)
                {
                    editor.content = text_editor::Content::with_text(&text);
                    editor.preview = markdown::Content::parse(&text);
                }
            }
            TaskTarget::Note { group, note } => {
                let Some((path, shown)) = self.note(group, note) else {
                    return;
                };
                let Some(body) = note_view::toggle_task(&shown.note.body, task, done) else {
                    return;
                };
                let (path, note) = (
                    path.clone(),
                    Note {
                        body,
                        ..shown.note.clone()
                    },
                );
                match self.store.save_note(&path, note) {
                    Ok(_) => {
                        self.error = None;
                        self.reload();
                    }
                    Err(e) => self.error = Some(format!("Couldn't save the note: {e}")),
                }
            }
        }
    }

    fn note(&self, group: usize, note: usize) -> Option<(&PathBuf, &ShownNote)> {
        let group = self.groups.get(group)?;
        Some((&group.path, group.notes.get(note)?))
    }

    fn report(&mut self, result: std::io::Result<()>) {
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
    }

    fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let notes = self.notes_view();
        let mut body = column![self.header(), rule::horizontal(1), notes];
        for notice in &self.notices {
            body = body.push(
                container(
                    row![
                        text(notice.text()).size(12).width(Fill),
                        button(text("Dismiss").size(11))
                            .style(button::text)
                            .on_press(Message::Dismiss(*notice)),
                    ]
                    .spacing(6)
                    .align_y(iced::Center),
                )
                .padding([6, 12])
                .width(Fill)
                .style(container::secondary),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(
                container(text(error).size(12).style(text::danger))
                    .padding([6, 12])
                    .width(Fill),
            );
        }
        container(body).style(container::bordered_box).into()
    }

    fn header(&self) -> Element<'_, Message> {
        let title = mouse_area(
            container(text("Hitch").size(15).font(BOLD))
                .padding([10, 12])
                .width(Fill),
        )
        .on_press(Message::Drag);

        let small = |label: &'static str| text(label).size(12);
        let pin = button(small(if self.state.pinned { "Pinned" } else { "Pin" }))
            .style(if self.state.pinned {
                button::primary
            } else {
                button::text
            })
            .on_press(Message::TogglePin);

        row![
            title,
            hint(
                button(small("+ Note"))
                    .style(button::text)
                    .on_press(Message::NewNote),
                format!("Add a note ({MOD}+Shift+H)"),
            ),
            hint(pin, format!("Keep on top of other windows ({MOD}+Shift+P)")),
            hint(
                button(small("Hide"))
                    .style(button::text)
                    .on_press(Message::Hide),
                format!("Hide the panel ({MOD}+Shift+N)"),
            ),
        ]
        .spacing(2)
        .padding(iced::Padding::ZERO.right(6))
        .align_y(iced::Center)
        .into()
    }

    fn notes_view(&self) -> Element<'_, Message> {
        let settings = markdown::Settings::with_text_size(TEXT_SIZE, self.markdown_theme());
        let mut list = column![].spacing(18).padding(12);

        if let Some(Editor {
            target: Target::New { choice },
            ..
        }) = &self.editor
        {
            let hitching = row![
                text("Hitching to:").size(12),
                pick_list(&self.choices[..], Some(choice), Message::HitchTo).text_size(12),
            ]
            .spacing(8)
            .align_y(iced::Center);
            list = list.push(column![hitching, self.editor_view(settings)].spacing(8));
        }

        let groups: Vec<_> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| !g.notes.is_empty())
            .collect();

        if groups.is_empty() && self.editor.is_none() {
            return center(
                text(format!(
                    "No notes here yet. Press {MOD}+Shift+H to add one."
                ))
                .size(TEXT_SIZE)
                .style(text::secondary)
                .center(),
            )
            .padding(24)
            .into();
        }

        for (gi, group) in groups {
            let heading = row![
                text(&group.label).size(13).font(BOLD),
                text(group.kind).size(11).style(text::secondary),
            ]
            .spacing(6)
            .align_y(iced::Center);
            let mut section = column![heading].spacing(14);
            for (ni, shown) in group.notes.iter().enumerate() {
                section = section.push(self.note_view(gi, ni, shown, settings));
            }
            list = list.push(section);
        }

        scrollable(list).height(Fill).into()
    }

    fn note_view<'a>(
        &'a self,
        group: usize,
        note: usize,
        shown: &'a ShownNote,
        settings: markdown::Settings,
    ) -> Element<'a, Message> {
        if let Some(Editor {
            target: Target::Existing { note: edited, path },
            ..
        }) = &self.editor
            && edited.id.is_some()
            && edited.id == shown.note.id
            && *path == self.groups[group].path
        {
            return self.editor_view(settings);
        }

        let title = row![
            text(&shown.note.title).size(15).font(BOLD).width(Fill),
            button(text("Edit").size(12))
                .style(button::text)
                .padding([2, 6])
                .on_press(Message::Edit { group, note }),
        ]
        .align_y(iced::Center);

        let mut card = column![title].spacing(6);
        if !shown.note.body.trim().is_empty() {
            let viewer = NoteViewer::new(TaskTarget::Note { group, note }, shown.first_code_block);
            card = card.push(viewer.view(shown.preview.items(), settings));
        }
        card.into()
    }

    fn editor_view(&self, settings: markdown::Settings) -> Element<'_, Message> {
        let Some(editor) = &self.editor else {
            return space().into();
        };
        let input = text_editor(&editor.content)
            .id(EDITOR_ID)
            .placeholder("## Title\n\nWrite the note in Markdown")
            .on_action(Message::EditorAction)
            .size(TEXT_SIZE)
            .min_height(120)
            .max_height(280);

        let preview = NoteViewer::new(TaskTarget::Editor, 0).view(editor.preview.items(), settings);

        let mut actions = row![
            button(text("Save").size(12))
                .style(button::primary)
                .on_press(Message::SaveEdit),
            button(text("Discard").size(12))
                .style(button::secondary)
                .on_press(Message::DiscardEdit),
            space::horizontal(),
        ]
        .spacing(6);
        if matches!(editor.target, Target::Existing { .. }) {
            actions = actions.push(
                button(
                    text(if editor.confirm_delete {
                        "Really delete?"
                    } else {
                        "Delete"
                    })
                    .size(12),
                )
                .style(button::danger)
                .on_press(Message::DeleteNote),
            );
        }

        container(
            column![
                input,
                text(format!("Esc or {MOD}+S saves"))
                    .size(11)
                    .style(text::secondary),
                actions,
                rule::horizontal(1),
                preview,
            ]
            .spacing(8),
        )
        .padding(8)
        .style(container::rounded_box)
        .into()
    }

    /// The theme the Markdown preview is styled for.
    fn markdown_theme(&self) -> Theme {
        match self.mode {
            theme::Mode::Dark => Theme::Dark,
            _ => Theme::Light,
        }
    }
}

const EDITOR_ID: &str = "hitch-note-editor";

fn hint<'a>(content: impl Into<Element<'a, Message>>, tip: String) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(tip).size(12))
            .padding([4, 8])
            .style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .into()
}

fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

fn dock_right(window: Size, monitor: Size) -> Point {
    Point::new(
        (monitor.width - window.width - DOCK_MARGIN).max(0.0),
        DOCK_MARGIN,
    )
}

fn dock_left(_window: Size, _monitor: Size) -> Point {
    Point::new(DOCK_MARGIN, DOCK_MARGIN)
}

fn on_event(event: Event, _status: event::Status, id: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            shortcut(key.as_ref(), modifiers).map(Message::Shortcut)
        }
        Event::Window(event) => {
            let event = match event {
                window::Event::Focused => WindowEvent::Focused,
                window::Event::Unfocused => WindowEvent::Unfocused,
                window::Event::CloseRequested => WindowEvent::CloseRequested,
                window::Event::Closed => WindowEvent::Closed,
                _ => return None,
            };
            Some(Message::Window(id, event))
        }
        _ => None,
    }
}

/// Maps a key press in the panel to a shortcut. Ctrl on Linux, Cmd on
/// macOS.
fn shortcut(key: Key<&str>, modifiers: Modifiers) -> Option<Shortcut> {
    match key {
        Key::Named(key::Named::Escape) if modifiers.is_empty() => Some(Shortcut::Escape),
        Key::Character(c) if modifiers.command() && !modifiers.alt() => {
            match (modifiers.shift(), c.to_ascii_lowercase().as_str()) {
                (true, "n") => Some(Shortcut::TogglePanel),
                (true, "h") => Some(Shortcut::Capture),
                (true, "p") => Some(Shortcut::TogglePin),
                (false, "s") => Some(Shortcut::Save),
                (false, "q") => Some(Shortcut::Quit),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Opens a link from a note in the default browser or mail app. Only web
/// and mail links: a HITCH.md is shared through git, so a link in it
/// shouldn't be able to launch arbitrary handlers.
fn open_url(url: &str) -> std::io::Result<()> {
    let allowed = ["https://", "http://", "mailto:"];
    if !allowed.iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
    }) {
        return Err(std::io::Error::other(format!(
            "Hitch only opens web and mail links, not {url}"
        )));
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    let mut child = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Reap it so it doesn't linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> Modifiers {
        if cfg!(target_os = "macos") {
            Modifiers::LOGO
        } else {
            Modifiers::CTRL
        }
    }

    #[test]
    fn shortcuts() {
        let ctrl_shift = ctrl() | Modifiers::SHIFT;
        assert_eq!(
            shortcut(Key::Character("P"), ctrl_shift),
            Some(Shortcut::TogglePin)
        );
        assert_eq!(
            shortcut(Key::Character("n"), ctrl_shift),
            Some(Shortcut::TogglePanel)
        );
        assert_eq!(
            shortcut(Key::Character("H"), ctrl_shift),
            Some(Shortcut::Capture)
        );
        assert_eq!(shortcut(Key::Character("s"), ctrl()), Some(Shortcut::Save));
        assert_eq!(shortcut(Key::Character("q"), ctrl()), Some(Shortcut::Quit));
        assert_eq!(
            shortcut(Key::Named(key::Named::Escape), Modifiers::empty()),
            Some(Shortcut::Escape)
        );
        // Plain typing and Ctrl+P (no Shift) are left to the editor.
        assert_eq!(shortcut(Key::Character("p"), Modifiers::SHIFT), None);
        assert_eq!(shortcut(Key::Character("p"), ctrl()), None);
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("-x").is_err());
    }

    #[test]
    fn docks_inside_the_monitor() {
        let p = dock_right(Size::new(320.0, 560.0), Size::new(1920.0, 1080.0));
        assert_eq!(p, Point::new(1920.0 - 320.0 - DOCK_MARGIN, DOCK_MARGIN));
        assert_eq!(
            dock_right(Size::new(320.0, 560.0), Size::new(200.0, 200.0)).x,
            0.0
        );
    }
}
