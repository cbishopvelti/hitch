//! The panel, built with iced.
//!
//! Hitch is one narrow floating panel (spec, UX). It lists the notes for the
//! active contexts, one section per notes file, most specific first, renders
//! them as Markdown and edits them in place.
//!
//! - **Show and hide**: the panel is a window Hitch opens and closes; the
//!   app keeps running with no window (an iced daemon). `hitch toggle`, or
//!   Ctrl+Shift+N while the panel has focus, flips it; `hitch toggle`
//!   raises a panel that is open behind other windows. Closing the window
//!   only hides it; Ctrl+Q quits.
//! - **Pin**: keeps the panel above other windows until unpinned, and is
//!   remembered across restarts. Unpinned, it is an ordinary window that
//!   stays open behind others and is listed in Alt+Tab. On X11 and macOS the window level does this; on Wayland
//!   only the shell can, so the context engine asks the GNOME extension
//!   (`SetKeepAbove`, see docs/PROTOCOL.md).
//! - **Move and resize**: the panel has no window frame, so it draws its
//!   own. Dragging the top bar moves it, and dragging an edge or corner
//!   resizes it; both hand the pointer to the window manager. The size is
//!   remembered across restarts. On macOS AppKit resizes a frameless
//!   window at its edges itself.
//! - **Notes**: read from and written to the HITCH.md files through
//!   `hitch-store`, and re-read when a shown file changes on disk. Each
//!   note is an H2 section. "Edit" opens it as Markdown with
//!   a live preview; Esc or Ctrl+S saves. "+ Note" (or `hitch capture`,
//!   Ctrl+Shift+H) starts a new note hitched to the most specific context.
//!   Its "Save to" dropdown (spec F14) lists the folders it can go into,
//!   nearest first, starting on the project folder or the last choice made
//!   here, and warns when a folder is above the file's git repo. Task checkboxes toggle in place.
//! - **Copy and send** (spec F12, F13): every note and code block has a
//!   Copy button; a code block's copies its code without the fences. Code
//!   blocks also have Send to Zellij, which types the code into the last
//!   focused Zellij pane unrun (Shift+click runs it; see [`zellij`]). With
//!   the panel focused and no text field taking keys, 1 to 9 copy the code
//!   blocks in order and Shift+1 to 9 send them.

mod backend;
mod label;
mod note_view;
mod panel_state;
mod zellij;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

use backend::{Engine, NoteGroup, Store, Watcher};
use note_view::{Done, NoteViewer, Spot, TaskTarget};
use panel_state::{Edge, PanelSize, PanelState, SavedChoice};

/// Panel size from the spec: about 320 px wide.
const PANEL_WIDTH: f32 = 320.0;
const PANEL_HEIGHT: f32 = 560.0;
/// The smallest the panel can be resized to.
const MIN_WIDTH: f32 = 240.0;
const MIN_HEIGHT: f32 = 200.0;
/// How far in from the panel's edges a drag resizes it, and how big the
/// corners that resize both ways are.
#[cfg(not(target_os = "macos"))]
const GRIP: f32 = 5.0;
#[cfg(not(target_os = "macos"))]
const CORNER: f32 = 14.0;
/// Gap between the docked panel and the screen edge (and the top bar).
const DOCK_MARGIN: f32 = 48.0;
const TEXT_SIZE: u32 = 14;
/// How long a button says "Copied" or "Sent".
const FLASH: Duration = Duration::from_millis(1500);

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
    let store = Store::for_user().ok_or("HOME is not set")?;
    // iced asks for a boot function it may call more than once; the engine
    // and store are handed over on the first call.
    let parts = std::cell::Cell::new(Some((engine, store)));
    let boot = move || {
        let (engine, store) = parts.take().expect("Hitch boots once");
        Panel::boot(engine, store)
    };
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
    watcher: Watcher,
    contexts: ActiveContexts,
    groups: Vec<Group>,
    /// Where a new note can be saved, nearest first.
    choices: Vec<Choice>,
    /// The choice a new note starts on when there's none remembered: the
    /// project folder.
    default_choice: usize,
    editor: Option<Editor>,
    mode: theme::Mode,
    /// The last thing that went wrong, shown under the notes until the next
    /// action succeeds.
    error: Option<String>,
    /// Setup hints from the engine, until dismissed.
    notices: Vec<Notice>,
    /// Hitch's own clipboard, kept for as long as Hitch runs (see
    /// Cargo.toml). `None` if it couldn't start; iced's is used then.
    clipboard: Option<arboard::Clipboard>,
    /// A Zellij session has reported, so code blocks can be sent to it.
    can_send: bool,
    /// The modifier keys held now, for Shift+click on Send.
    modifiers: Modifiers,
    /// The button that just copied or sent, and which flash this is.
    flash: Option<(Spot, Done)>,
    flash_count: u64,
    /// The panel was resized since its size was last saved.
    size_changed: bool,
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
    /// Whether it has focus now, so the toggle shortcut raises a panel that
    /// is behind other windows instead of closing it.
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

/// A place a new note can be saved to (spec F14).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The context the note is saved under. For a file this is the file,
    /// relative to the chosen folder, so the note stays hitched to it.
    context: Context,
    /// The notes file it goes into. It need not exist yet.
    path: PathBuf,
    label: String,
    /// Shown before saving when the folder is above the file's git repo.
    warning: Option<String>,
}

impl Choice {
    /// How the choice is remembered in panel.json.
    fn saved(&self) -> SavedChoice {
        SavedChoice {
            path: self.path.clone(),
            file: matches!(self.context, Context::File { .. }),
        }
    }
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
    /// A watched notes file changed on disk.
    NotesChanged,
    Window(window::Id, WindowEvent),
    Shortcut(Shortcut),
    ThemeChanged(theme::Mode),
    Drag,
    /// An edge or corner was pressed: resize the panel that way. Not on
    /// macOS, which has no grips (see [`with_resize_grips`]).
    #[cfg(not(target_os = "macos"))]
    Resize(window::Direction),
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
    /// Copy `text`; the button at `spot` says "Copied".
    Copy {
        text: String,
        spot: Spot,
    },
    /// Type `text` into the last focused Zellij pane; Shift+click runs it.
    SendToZellij {
        text: String,
        spot: Spot,
    },
    Sent {
        spot: Spot,
        result: Result<(), String>,
    },
    /// The flash numbered this is over.
    FlashOver(u64),
    ModifiersChanged(Modifiers),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    Focused,
    Unfocused,
    CloseRequested,
    Closed,
    /// The window's new size, in logical pixels.
    Resized(PanelSize),
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
    /// 1 to 9: copy the code block with this number, counting from 0.
    CopyBlock(usize),
    /// Shift+1 to 9: send it to Zellij, unrun.
    SendBlock(usize),
}

impl Panel {
    fn boot(engine: Engine, store: Store) -> (Self, Task<Message>) {
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
            watcher: Watcher::new(store.config()),
            store,
            groups: Vec::new(),
            choices: Vec::new(),
            default_choice: 0,
            editor: None,
            mode: theme::Mode::None,
            error: None,
            notices: Vec::new(),
            clipboard: arboard::Clipboard::new().ok(),
            can_send: false,
            modifiers: Modifiers::empty(),
            flash: None,
            flash_count: 0,
            size_changed: false,
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
            self.watcher.subscription().map(|_| Message::NotesChanged),
            event::listen_with(on_event),
            system::theme_changes().map(Message::ThemeChanged),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Engine(backend::Event::Contexts(contexts)) => {
                self.can_send = self.engine.zellij_target().is_some();
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
            // Only the shown notes files are watched, so any change is ours.
            Message::NotesChanged => {
                self.reload();
                Task::none()
            }
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
            #[cfg(not(target_os = "macos"))]
            Message::Resize(direction) => match self.window {
                Some(w) => window::drag_resize(w.id, direction),
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
            Message::Copy { text, spot } => self.copy(text, spot),
            Message::SendToZellij { text, spot } => {
                self.send_to_zellij(text, spot, self.modifiers.shift())
            }
            Message::Sent { spot, result } => match result {
                Ok(()) => {
                    self.error = None;
                    self.flash(spot, Done::Sent)
                }
                Err(e) => {
                    self.error = Some(e);
                    Task::none()
                }
            },
            Message::FlashOver(count) => {
                if count == self.flash_count {
                    self.flash = None;
                }
                Task::none()
            }
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
        }
    }

    /// Puts `text` on the clipboard.
    fn copy(&mut self, text: String, spot: Spot) -> Task<Message> {
        let copied = match &mut self.clipboard {
            Some(clipboard) => clipboard.set_text(text),
            None => {
                return Task::batch([iced::clipboard::write(text), self.flash(spot, Done::Copied)]);
            }
        };
        match copied {
            Ok(()) => {
                self.error = None;
                self.flash(spot, Done::Copied)
            }
            Err(e) => {
                self.error = Some(format!("Couldn't copy: {e}"));
                Task::none()
            }
        }
    }

    /// Types `text` into the last focused Zellij pane; with `submit`, also
    /// presses Enter.
    fn send_to_zellij(&mut self, text: String, spot: Spot, submit: bool) -> Task<Message> {
        if self.engine.deliver_write_chars(&text, submit) {
            self.error = None;
            return self.flash(spot, Done::Sent);
        }
        let Some(target) = self.engine.zellij_target() else {
            self.error = Some(
                "No Zellij session has reported to Hitch yet. Load the Hitch plugin in Zellij, \
                 or focus a Zellij pane."
                    .into(),
            );
            return Task::none();
        };
        Task::perform(
            off_thread(move || zellij::Zellij::default().send(&target, &text, submit)),
            move |result| Message::Sent {
                spot,
                result: result
                    .unwrap_or_else(|| Err("Sending to Zellij failed.".into()))
                    .map(drop),
            },
        )
    }

    /// Makes the button at `spot` say what it did, for a moment.
    fn flash(&mut self, spot: Spot, done: Done) -> Task<Message> {
        self.flash = Some((spot, done));
        self.flash_count += 1;
        let count = self.flash_count;
        Task::perform(off_thread(|| std::thread::sleep(FLASH)), move |_| {
            Message::FlashOver(count)
        })
    }

    /// The code block with panel-wide number `number` (from 0): its text,
    /// and where its buttons are.
    fn code_block(&self, number: usize) -> Option<(String, Spot)> {
        for (gi, group) in self.groups.iter().enumerate() {
            for (ni, shown) in group.notes.iter().enumerate() {
                let Some(index) = number.checked_sub(shown.first_code_block) else {
                    continue;
                };
                if let Some(code) = note_view::code_blocks(shown.preview.items()).get(index) {
                    let target = TaskTarget::Note {
                        group: gi,
                        note: ni,
                    };
                    return Some((note_view::block_text(code), Spot::Block { target, index }));
                }
            }
        }
        None
    }

    fn command(&mut self, command: Command) -> Task<Message> {
        match command {
            // Hides a focused panel; raises one that is behind other windows.
            Command::TogglePanel if self.window.is_some_and(|w| w.focused) => self.hide(),
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
                self.save_size();
                iced::exit()
            }
            Shortcut::CopyBlock(number) => match self.code_block(number) {
                Some((text, spot)) => self.copy(text, spot),
                None => Task::none(),
            },
            Shortcut::SendBlock(number) => match self.code_block(number) {
                Some((text, spot)) if self.can_send => self.send_to_zellij(text, spot, false),
                _ => Task::none(),
            },
        }
    }

    fn window_event(&mut self, id: window::Id, event: WindowEvent) -> Task<Message> {
        let Some(window) = self.window.as_mut().filter(|w| w.id == id) else {
            return Task::none();
        };
        match event {
            // The panel is an ordinary window: losing focus leaves it open
            // behind other windows, listed in Alt+Tab (spec, UX).
            WindowEvent::Focused => {
                window.focused = true;
                Task::none()
            }
            WindowEvent::Unfocused => {
                window.focused = false;
                Task::none()
            }
            WindowEvent::CloseRequested => self.hide(),
            WindowEvent::Closed => {
                self.window = None;
                self.save_size();
                Task::none()
            }
            WindowEvent::Resized(size) => {
                if size != self.panel_size() {
                    self.state.size = Some(size);
                    self.size_changed = true;
                }
                Task::none()
            }
        }
    }

    /// The panel's size: the last one it was resized to, or the default.
    fn panel_size(&self) -> PanelSize {
        self.state.size.unwrap_or(PanelSize {
            width: PANEL_WIDTH as u32,
            height: PANEL_HEIGHT as u32,
        })
    }

    /// Saves a size changed by resizing. It's saved when the panel hides
    /// or Hitch quits, not on every step of a resize.
    fn save_size(&mut self) {
        if !std::mem::take(&mut self.size_changed) {
            return;
        }
        if let Some(path) = &self.state_path {
            let result = self.state.save(path);
            self.report(result);
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
        self.save_size();
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
        let size = self.panel_size();
        window::Settings {
            size: Size::new(
                (size.width as f32).max(MIN_WIDTH),
                (size.height as f32).max(MIN_HEIGHT),
            ),
            min_size: Some(Size::new(MIN_WIDTH, MIN_HEIGHT)),
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
        self.can_send = self.engine.zellij_target().is_some();
        (self.choices, self.default_choice) = save_choices(&self.contexts.contexts, &self.store);
        self.watcher
            .watch_only(self.choices.iter().map(|c| c.path.clone()));

        // A note just saved to a folder between the file and its project
        // makes that folder the project; show its notes before the engine
        // next resolves the file.
        let inner = self.choices.iter().filter_map(|c| match &c.context {
            Context::File { project, .. } if c.path.is_file() => self
                .contexts
                .contexts
                .iter()
                .any(|ctx| {
                    matches!(ctx, Context::File { project: outer, .. }
                        if project != outer && project.starts_with(outer))
                })
                .then(|| c.context.clone()),
            _ => None,
        });
        let shown: Vec<Context> = inner.chain(self.contexts.contexts.clone()).collect();
        let groups = match self.store.notes_for_all(&shown) {
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
        let start = self.start_choice();
        if let Some(Editor {
            target: Target::New { choice },
            ..
        }) = &mut self.editor
            && !self.choices.contains(choice)
            && let Some(start) = start
        {
            *choice = start;
        }
    }

    fn start_new_note(&mut self) -> Task<Message> {
        self.commit_edit();
        let Some(choice) = self.start_choice() else {
            self.error = Some("Nothing to hitch a note to yet.".into());
            return Task::none();
        };
        self.open_editor(Target::New { choice }, "## ")
    }

    /// The choice a new note starts on: the last one used for these
    /// contexts, else the project folder.
    fn start_choice(&self) -> Option<Choice> {
        let default = self.choices.get(self.default_choice)?;
        self.state
            .save_to
            .get(&remember_key(default))
            .and_then(|saved| self.choices.iter().find(|c| c.saved() == *saved))
            .or(Some(default))
            .cloned()
    }

    /// Remembers `choice` as the place new notes go for the current
    /// contexts (spec F14: the last choice per project).
    fn remember_choice(&mut self, choice: &Choice) {
        let Some(default) = self.choices.get(self.default_choice) else {
            return;
        };
        let key = remember_key(default);
        let saved = choice.saved();
        if self.state.save_to.get(&key) == Some(&saved) {
            return;
        }
        if saved == default.saved() {
            self.state.save_to.remove(&key);
        } else {
            self.state.save_to.insert(key, saved);
        }
        if let Some(path) = &self.state_path {
            let result = self.state.save(path);
            self.report(result);
        }
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
                let note = new_note(&choice.context, title, body);
                let saved = self.store.save_note(&choice.path, note).map(drop);
                if saved.is_ok() {
                    self.remember_choice(&choice.clone());
                }
                saved
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
        with_resize_grips(container(body).style(container::bordered_box).into())
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
            let save_to = row![
                text("Save to:").size(12),
                pick_list(&self.choices[..], Some(choice), Message::HitchTo)
                    .text_size(12)
                    .width(Fill),
            ]
            .spacing(8)
            .align_y(iced::Center);
            let mut capture = column![save_to].spacing(8);
            if let Some(warning) = &choice.warning {
                capture = capture.push(text(warning).size(12).style(text::warning));
            }
            list = list.push(capture.push(self.editor_view(settings)));
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

        let spot = Spot::Note { group, note };
        let copied = self.flash == Some((spot, Done::Copied));
        let mut title =
            row![text(&shown.note.title).size(15).font(BOLD).width(Fill)].align_y(iced::Center);
        if !shown.note.body.trim().is_empty() {
            title = title.push(hint(
                button(text(if copied { "Copied" } else { "Copy" }).size(12))
                    .style(button::text)
                    .padding([2, 6])
                    .on_press(Message::Copy {
                        text: shown.note.body.clone(),
                        spot,
                    }),
                "Copy the note's text".into(),
            ));
        }
        title = title.push(
            button(text("Edit").size(12))
                .style(button::text)
                .padding([2, 6])
                .on_press(Message::Edit { group, note }),
        );

        let mut card = column![title].spacing(6);
        if !shown.note.body.trim().is_empty() {
            let viewer = NoteViewer::new(TaskTarget::Note { group, note }, shown.first_code_block)
                .buttons(self.can_send, self.flash);
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

        let preview = NoteViewer::new(TaskTarget::Editor, 0)
            .buttons(self.can_send, self.flash)
            .view(editor.preview.items(), settings);

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

/// Runs `f` on its own thread, for work that blocks (running `zellij`,
/// waiting). `None` if it panicked.
fn off_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl std::future::Future<Output = Option<T>> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    async move { rx.await.ok() }
}

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

/// Where a new note can go: the active contexts, most specific first, with
/// each file's project folder right after it, so making a note
/// project-wide is one pick away (spec, matching rule 4).
fn save_choices(contexts: &[Context], store: &Store) -> (Vec<Choice>, usize) {
    let file_name = store.file_name();
    // Repo warnings are measured from the most specific file or folder.
    let anchor = contexts.iter().find_map(|c| match c {
        Context::File { project, file } => Some(project.join(file)),
        Context::Folder { path } => Some(path.clone()),
        _ => None,
    });
    let warning = |folder: &Path| {
        let repo = hitch_store::outside_repo(folder, anchor.as_deref()?)?;
        let name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| label::tilde(&repo));
        Some(format!(
            "{} is outside the {name} repo, so this note won't be committed with it.",
            label::tilde(folder)
        ))
    };
    let folder_label = |folder: &Path, extra: &str| {
        let new = if folder.join(file_name).is_file() {
            ""
        } else {
            " (new)"
        };
        format!("{}{extra}{new}", label::tilde(folder))
    };

    let mut choices: Vec<Choice> = Vec::new();
    let mut default = None;
    for context in contexts {
        match context {
            Context::File { project, file } => {
                let abs = project.join(file);
                for folder in hitch_store::save_folders(&abs, project, file_name) {
                    let file = hitch_store::relative_to(&abs, &folder.path)
                        .unwrap_or_else(|| file.clone());
                    let context = Context::File {
                        project: folder.path.clone(),
                        file,
                    };
                    if choices.iter().any(|c| c.context == context) {
                        continue;
                    }
                    let is_project = folder.path == *project;
                    if is_project && default.is_none() {
                        default = Some(choices.len());
                    }
                    choices.push(Choice {
                        path: store.notes_path(&context),
                        label: folder_label(&folder.path, ""),
                        warning: warning(&folder.path),
                        context,
                    });
                    // Next to it, the project folder for the whole project
                    // rather than hitched to the file.
                    let whole = Context::Folder {
                        path: project.clone(),
                    };
                    if is_project && !choices.iter().any(|c| c.context == whole) {
                        choices.push(Choice {
                            path: store.notes_path(&whole),
                            label: folder_label(project, ", whole project"),
                            warning: warning(project),
                            context: whole,
                        });
                    }
                }
            }
            Context::Folder { path } => {
                let notes = store.notes_path(context);
                // A file's choices already cover this folder.
                if choices.iter().any(|c| c.path == notes) {
                    continue;
                }
                default.get_or_insert(choices.len());
                choices.push(Choice {
                    path: notes,
                    label: folder_label(path, ""),
                    warning: warning(path),
                    context: context.clone(),
                });
            }
            other => {
                if choices.iter().any(|c| c.context == *other) {
                    continue;
                }
                default.get_or_insert(choices.len());
                choices.push(Choice {
                    path: store.notes_path(other),
                    label: label::context(other),
                    warning: None,
                    context: other.clone(),
                });
            }
        }
    }
    (choices, default.unwrap_or(0))
}

/// The key a remembered choice is kept under: the default choice's notes
/// file, which stands for the project.
fn remember_key(default: &Choice) -> String {
    default.path.to_string_lossy().into_owned()
}

/// A new note for `context`. For a file it is hitched to that file (spec,
/// matching rule 4); picking the project folder instead makes it
/// project-wide.
fn new_note(context: &Context, title: String, body: String) -> Note {
    let mut note = Note::new(title, body);
    if let Context::File { file, .. } = context {
        note.files = vec![file.clone()];
    }
    note
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

/// Lays invisible strips over the panel's edges and corners that resize it:
/// the panel has no window frame, so nothing else would (on Wayland the
/// compositor draws no frame for it, and on X11 there is none to grab).
#[cfg(not(target_os = "macos"))]
fn with_resize_grips(panel: Element<'_, Message>) -> Element<'_, Message> {
    // Imported here, not at the top, so the macOS build has none unused.
    use iced::Length::Fixed;
    use iced::mouse;
    use iced::widget::stack;
    use iced::window::Direction;
    use mouse::Interaction::{
        ResizingDiagonallyDown as Nwse, ResizingDiagonallyUp as Nesw,
        ResizingHorizontally as Horizontal, ResizingVertically as Vertical,
    };
    fn grip<'a>(
        direction: Direction,
        interaction: mouse::Interaction,
        width: impl Into<iced::Length>,
        height: impl Into<iced::Length>,
    ) -> Element<'a, Message> {
        mouse_area(space().width(width).height(height))
            .interaction(interaction)
            .on_press(Message::Resize(direction))
            .into()
    }
    let corner =
        |direction, interaction| grip(direction, interaction, Fixed(CORNER), Fixed(CORNER));

    let edges = column![
        grip(Direction::North, Vertical, Fill, Fixed(GRIP)),
        row![
            grip(Direction::West, Horizontal, Fixed(GRIP), Fill),
            space().width(Fill),
            grip(Direction::East, Horizontal, Fixed(GRIP), Fill),
        ]
        .height(Fill),
        grip(Direction::South, Vertical, Fill, Fixed(GRIP)),
    ];
    let corners = column![
        row![
            corner(Direction::NorthWest, Nwse),
            space().width(Fill),
            corner(Direction::NorthEast, Nesw),
        ],
        space().height(Fill),
        row![
            corner(Direction::SouthWest, Nesw),
            space().width(Fill),
            corner(Direction::SouthEast, Nwse),
        ],
    ];
    stack![panel, edges, corners].into()
}

/// AppKit resizes a frameless window from its edges itself, and winit
/// can't start a resize there.
#[cfg(target_os = "macos")]
fn with_resize_grips(panel: Element<'_, Message>) -> Element<'_, Message> {
    panel
}

fn on_event(event: Event, status: event::Status, id: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            ..
        }) => shortcut(key.as_ref(), modifiers)
            // Digits typed into the editor are the editor's.
            .or_else(|| {
                (status == event::Status::Ignored)
                    .then(|| block_shortcut(physical_key, modifiers))
                    .flatten()
            })
            .map(Message::Shortcut),
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::ModifiersChanged(modifiers))
        }
        Event::Window(event) => {
            let event = match event {
                window::Event::Focused => WindowEvent::Focused,
                window::Event::Unfocused => WindowEvent::Unfocused,
                window::Event::CloseRequested => WindowEvent::CloseRequested,
                window::Event::Closed => WindowEvent::Closed,
                window::Event::Resized(size) => WindowEvent::Resized(PanelSize {
                    width: size.width.round() as u32,
                    height: size.height.round() as u32,
                }),
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

/// 1 to 9 copy a code block and Shift+1 to 9 send it. Matched by the
/// physical key, so it works whatever the keyboard layout puts there.
fn block_shortcut(key: key::Physical, modifiers: Modifiers) -> Option<Shortcut> {
    use key::Code;
    if modifiers.command() || modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    let key::Physical::Code(code) = key else {
        return None;
    };
    let digits = [
        (Code::Digit1, Code::Numpad1),
        (Code::Digit2, Code::Numpad2),
        (Code::Digit3, Code::Numpad3),
        (Code::Digit4, Code::Numpad4),
        (Code::Digit5, Code::Numpad5),
        (Code::Digit6, Code::Numpad6),
        (Code::Digit7, Code::Numpad7),
        (Code::Digit8, Code::Numpad8),
        (Code::Digit9, Code::Numpad9),
    ];
    let number = digits
        .iter()
        .position(|(digit, numpad)| code == *digit || code == *numpad)?;
    Some(if modifiers.shift() {
        Shortcut::SendBlock(number)
    } else {
        Shortcut::CopyBlock(number)
    })
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
    fn number_keys_copy_and_send_code_blocks() {
        use key::{Code, Physical};
        let none = Modifiers::empty();
        assert_eq!(
            block_shortcut(Physical::Code(Code::Digit1), none),
            Some(Shortcut::CopyBlock(0))
        );
        assert_eq!(
            block_shortcut(Physical::Code(Code::Numpad9), none),
            Some(Shortcut::CopyBlock(8))
        );
        assert_eq!(
            block_shortcut(Physical::Code(Code::Digit3), Modifiers::SHIFT),
            Some(Shortcut::SendBlock(2))
        );
        assert_eq!(block_shortcut(Physical::Code(Code::Digit0), none), None);
        assert_eq!(block_shortcut(Physical::Code(Code::Digit1), ctrl()), None);
        assert_eq!(
            block_shortcut(Physical::Code(Code::Digit1), Modifiers::ALT),
            None
        );
        assert_eq!(block_shortcut(Physical::Code(Code::KeyA), none), None);
    }

    #[test]
    fn new_notes_are_hitched_to_the_current_file() {
        let file = Context::File {
            project: "/p".into(),
            file: "src/a.rs".into(),
        };
        let note = new_note(&file, "T".into(), "B".into());
        assert_eq!(note.files, vec![PathBuf::from("src/a.rs")]);
        assert!(note.id.is_none());

        let folder = Context::Folder { path: "/p".into() };
        assert!(new_note(&folder, "T".into(), "B".into()).is_project_wide());
    }

    #[test]
    fn save_to_lists_folders_nearest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let playground = root.join("playground");
        let boodle = playground.join("boodle");
        let src = boodle.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir(boodle.join(".git")).unwrap();
        std::fs::write(playground.join("HITCH.md"), "").unwrap();
        std::fs::write(src.join("main.rs"), "").unwrap();
        let store = Store::new(hitch_store::StoreConfig::new(root.join(".hitch")));

        let contexts = [
            Context::File {
                project: boodle.clone(),
                file: "src/main.rs".into(),
            },
            Context::Folder {
                path: boodle.clone(),
            },
            Context::Folder {
                path: playground.clone(),
            },
            Context::App {
                app_id: "dev.zed.Zed".into(),
            },
        ];
        let (choices, default) = save_choices(&contexts, &store);
        let targets: Vec<_> = choices.iter().map(|c| c.context.clone()).collect();
        assert_eq!(
            targets,
            vec![
                Context::File {
                    project: src.clone(),
                    file: "main.rs".into(),
                },
                Context::File {
                    project: boodle.clone(),
                    file: "src/main.rs".into(),
                },
                Context::Folder {
                    path: boodle.clone(),
                },
                Context::File {
                    project: playground.clone(),
                    file: "boodle/src/main.rs".into(),
                },
                Context::App {
                    app_id: "dev.zed.Zed".into(),
                },
            ]
        );
        assert_eq!(default, 1, "the project folder is the default");
        assert!(choices[0].label.ends_with(" (new)"));
        assert!(choices[..3].iter().all(|c| c.warning.is_none()));
        let warning = choices[3].warning.as_deref().unwrap();
        assert!(
            warning
                .ends_with("is outside the boodle repo, so this note won't be committed with it."),
            "{warning}"
        );
        assert_eq!(choices[3].path, playground.join("HITCH.md"));

        // A folder alone defaults to itself.
        let (choices, default) = save_choices(&contexts[1..3], &store);
        assert_eq!(choices.len(), 2);
        assert_eq!(default, 0);
        assert!(choices[1].warning.is_some());
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
