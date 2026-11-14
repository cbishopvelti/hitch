# Hitch: product spec

> Snapshot of the living spec at https://claude.ai/artifact/RnJzVBjY4wwgx4k3692Xtm, taken 2026-11-11. If the two differ, the living doc wins; update this copy in the same PR as any code that depends on the change.

## Overview

Hitch is a desktop app that shows the notes for whatever you are working on right now, and lets you add one in a keystroke. Notes are hitched to a context (an app, a window, a project folder, a file or a web page), so they reappear by themselves the next time that context is in front of you.

**The problem.** Working knowledge is scattered: the deploy command for one repo, the quirks of a client's admin portal, the half-finished thought about a document. General notes apps make you go and search for it. Hitch brings it to you.

**One-line pitch:** notes that follow your work, not the other way round.

This draft assumes "context" means a mix of app, window, folder, file and URL. That assumption is the first open question to confirm.

## Goals and non-goals

Version 1 succeeds if the right note is on screen within a second of switching context, and adding a note takes one shortcut and no filing decisions.

**Goals for v1**

- Detect the current context automatically on the latest Ubuntu first, then macOS.
- Show matching notes in a small panel that stays out of the way until wanted.
- Capture a note with a global shortcut, already hitched to the current context.
- Store notes as plain Markdown in a HITCH.md file inside the folder they belong to.
- Search across all notes when a note is needed outside its context.

**Non-goals for v1**

- Sync between devices or sharing with other people.
- Windows and mobile apps.
- Rich documents, tasks or a full knowledge base. Hitch holds short notes.
- AI features such as summaries or suggested notes.

## What a context is

The main context is a file on disk. Wherever Hitch can tell which file or project you are working on, it opens that project's HITCH.md and shows the notes for that file first. Each source below reports a file, a project, or both.

| Source | What it reports | Context Hitch uses | Priority |
|---|---|---|---|
| Zed, through the Hitch extension | Active file and project root | That file, inside its project | Must |
| Browser on a clawswap dev-server page | The project's source folder | That folder's HITCH.md | Must |
| Zellij, through the Hitch plugin | Focused pane's working folder | The project containing that folder | Must |
| Other apps | File path in the window title, when shown | That file, if it can be found on disk | Should |
| Apps and websites with no file | App id or URL | A fallback file in ~/.hitch/ | Could |

**Matching rules**

1. A file resolves to its project folder: the nearest folder with either a HITCH.md or a `.git` folder.
2. The panel shows notes tagged for the current file first, then the rest of the project's HITCH.md, then any HITCH.md in parent folders.
3. Switching to an app that reports no file, such as a chat app, keeps the last file's notes on screen. This is what makes a pinned panel useful while you work in another window.
4. A new note is tagged with the current file by default; one click makes it project-wide.
5. On a clawswap page, clawswap tells Hitch the project's source folder, and Hitch shows the HITCH.md there.

## User stories and key flows

**Stories**

- As a developer, when I open a repo I see the setup commands and gotchas I wrote last time.
- As a freelancer, when I open a client's portal I see their login quirks and billing rules.
- As a writer, when I reopen a draft I see the note I left about what to fix next.
- As anyone, I can jot a thought about what I'm looking at without leaving it.

**Flow 1: notes appear**

1. User switches to a window.
2. Hitch detects the context change (debounced by about 300 ms so fast switching doesn't flicker).
3. If the panel is open or pinned, it updates to the new context's notes.
4. If the user turned on "show on arrival" for a context, the panel slides in by itself.

**Flow 2: capture a note**

1. User presses the global shortcut (default Ctrl/Cmd+Shift+H).
2. A small editor opens over the current window, labelled with the context it will hitch to.
3. User types; Esc saves and closes, returning focus to the previous window.
4. Optional: change the context with a dropdown of all active contexts before saving.

**Flow 3: find a note elsewhere**

1. User opens search from the panel or a second shortcut.
2. Full-text search across all notes, with the context shown on each result.
3. Opening a result can jump to its context (open the folder, file or URL).

## Functional requirements (v1)

| ID | Requirement | Priority |
|---|---|---|
| F1 | Detect frontmost app and window title on the latest Ubuntu (Wayland), then macOS | Must |
| F2 | Get the open file, project root and working folder from a Hitch extension for Zed and a Hitch plugin for Zellij; fall back to the window title for other apps | Must |
| F3 | Through clawswap, get the source folder of the project whose dev-server page is in the browser | Must |
| F4 | Show notes for all active contexts, grouped, most specific first | Must |
| F5 | Global shortcut to capture a note hitched to the most specific context | Must |
| F6 | Edit, delete and re-hitch notes; hitch one note to several contexts | Must |
| F7 | Markdown editing with live preview, checkboxes and links | Must |
| F8 | Full-text search across all notes | Must |
| F9 | Per-context "show on arrival" toggle; pin the panel always on top | Should |
| F10 | Ignore list of apps, sites and folders that Hitch never records | Must |
| F11 | Import and export as a folder of Markdown files | Should |
| F12 | One-click copy: a copy button on every code block and note, and number keys 1 to 9 copy the panel's code blocks in order | Must |
| F13 | Send a code block to Zellij: typed into the last focused Zellij pane without pressing Enter; Shift+click runs it | Must |
| F14 | Choose on save which folder's HITCH.md a note goes into; warn when that folder is outside the file's git repo | Must |

**Non-functional**

- Context switch to notes on screen in under 200 ms for up to 10,000 notes.
- Idle CPU under 1% and memory under 150 MB.
- Works fully offline; no account needed.

## UX

Hitch is one floating panel, shown and hidden with a shortcut; it never takes over the screen. There is no tray icon and no launch at login.

- **Opening Hitch.** Started like any app; Ctrl/Cmd+Shift+N then shows or hides the panel.
- **Code blocks.** Each one shows two buttons on hover: Copy, and Send to Zellij (only when a Zellij session is running). Copy puts the exact text on the clipboard with no Markdown fences and briefly shows "Copied". Send types the command into the last focused Zellij pane and leaves it unrun so it can be checked; Shift+click also presses Enter. With the panel focused, keys 1 to 9 copy code blocks in order, and Shift with the number sends them.
- **Panel.** A narrow floating window (about 320 px wide by default) docked to a screen edge the user chooses. Dragging its top bar moves it and dragging an edge or corner resizes it; the size is remembered across restarts. Sections per active context, each with its label (app icon, folder name, domain). A pin button (and shortcut Ctrl/Cmd+Shift+P) keeps the panel always on top of other windows, on every desktop or Space, until unpinned; unpinned, it auto-hides. The pin state is remembered across restarts. On Ubuntu Wayland, apps cannot set always-on-top themselves, so the GNOME extension does it for Hitch.
- **Quick capture.** A compact editor that opens near the cursor, with a "Save to" picker beside the text and saves on Esc. The picker lists the folders the note can go into, nearest first: each folder from the file's own folder up to its project folder (the default), then any folder further up that already has a HITCH.md. Choosing a folder with no HITCH.md creates one there on save. If the chosen folder is above the git repo the file is in, the editor shows a warning before saving, e.g. "~/playground is outside the boodle repo, so this note won't be committed with it." Saving still works; the warning just makes the choice deliberate. The last choice per project is remembered.
- **Empty state.** "No notes here yet. Press Ctrl/Cmd+Shift+H to add one."
- **Permissions onboarding.** On first run on macOS, a short screen explains why Accessibility and Automation access are needed and opens the right System Settings pane. On Ubuntu with Wayland, it offers to install the small GNOME extension Hitch needs (see Architecture).
- **Settings.** Shortcuts, panel position and behaviour, notes file name, fallback folder location, ignore list, and the clawswap daemon address.

## Data model and storage

Notes live next to the work. Each folder context keeps its notes in one Markdown file, `HITCH.md`, at the folder's root. Opening ~/playground/boodle reads and writes ~/playground/boodle/HITCH.md, so the notes travel with the project and can be edited in any editor.

| Context | Where its notes are stored |
|---|---|
| Project folder | `<folder>/HITCH.md` |
| File | The HITCH.md of its project folder, in a section tagged with the file's relative path |
| App, web site, web page, window title | A fallback folder, one file per context, e.g. `~/.hitch/apps/figma.md` |

**Finding the project folder.** Hitch walks up from the current path and picks the nearest folder that has a HITCH.md or a `.git` folder, and failing that the folder itself. So a git repo with no notes yet is still its own project, rather than adding its first note to a HITCH.md further up. Every HITCH.md on the way up is shown too, so notes in ~/playground also appear inside ~/playground/boodle.

```markdown
# Hitch notes

## Dev setup
<!-- hitch id=01JC8Z6Q3N4V created=2026-11-11 -->
Run `cargo run`; the first build takes a few minutes.

## Parser bug
<!-- hitch id=01JC8Z9K2B7M file=src/parser.rs created=2026-11-11 -->
Fails on empty front matter.
```

Each note is an H2 section. The comment line holds Hitch's metadata and is optional, so a section written by hand with no comment still counts as a note for the folder.

| Store | What it holds | Where |
|---|---|---|
| HITCH.md | Notes for one folder and the files in it (source of truth) | Each project folder |
| Fallback files | Notes for apps, sites, pages and window titles | ~/.hitch/ |
| Known locations | Path of every HITCH.md Hitch has seen | ~/.hitch/locations.json |
| Index | Notes, contexts and a full-text table (FTS5) | SQLite cache, rebuilt from the files |
| Settings | Shortcuts, panel, ignore list, file name | ~/.hitch/settings.json |

Search covers every known HITCH.md plus the fallback folder. A file watcher on each known location picks up edits made outside Hitch, including a git pull. The file name is a setting: HITCH.md by default, which can be changed per user. HITCH.md is meant to be committed with the project, so the notes are versioned and shared with anyone who clones it. Hitch never adds it to .gitignore, and it keeps the file's formatting stable so diffs stay small.

## Architecture and platform

Hitch is a native Rust app with no web view: one binary for the core and the UI. For the UI, iced is the recommended toolkit: it is pure Rust, runs on Wayland and macOS, and has a Markdown widget to build the preview on. GTK4 through gtk-rs is the fallback; it looks most at home on Ubuntu's GNOME but is weaker on macOS. Ubuntu's latest releases no longer ship an X11 session, so Wayland is the target and X11 support is a bonus.

Context detection feeds a local index; nothing leaves the machine.

```text
 ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
 │ Context watcher  │──▶│ Context resolver │──▶│ Matcher          │──▶│ Panel and capture│
 │ OS APIs + plugins│   │ ignore list first│   │ looks up notes   │   │ native Rust UI   │
 │ emits changes    │   │ derives contexts │   │ most specific 1st│   │ shows and edits  │
 └──────────────────┘   └──────────────────┘   └──────────────────┘   └──────────────────┘
                                                 ▲ query                 │ saves
                                               ┌──────────────────┐   ┌──▼───────────────┐
                                               │ SQLite index     │◀──│ HITCH.md files   │
                                               │ contexts, full   │   │ one per project  │
                                               │ text; disposable │   │ source of truth  │
                                               └──────────────────┘   └──────────────────┘
                                                        file watcher re-indexes
```

The context watcher is the riskiest part to build (per-OS detection).

The watcher reports changes, the resolver turns them into contexts, and the panel reads notes through the index; the Markdown files stay the source of truth.

| Signal | macOS | Ubuntu, Wayland (the default) | Ubuntu, X11 |
|---|---|---|---|
| Frontmost app | NSWorkspace activation notifications | Companion GNOME Shell extension reports the focused window over D-Bus | _NET_ACTIVE_WINDOW and WM_CLASS |
| Window title | Accessibility API (needs Accessibility permission) | Same extension | _NET_WM_NAME |
| Open file and project root | Hitch extension for Zed; title parsing for other apps | Same as macOS | Same as macOS |
| Terminal folder | Hitch plugin for Zellij reports the focused pane's folder | Same as macOS | Same as macOS |
| Local dev pages | clawswap reports the project's source folder | Same as macOS | Same as macOS |
| Global shortcut | Native global hotkey API | XDG GlobalShortcuts portal, or a GNOME custom shortcut that runs `hitch toggle` | XGrabKey |

**Integrations.** Zed and Zellij report context to Hitch over a local socket (`~/.hitch/hitch.sock`) as a small JSON message: source, project root, file, working folder. A plugin report is exact, so it wins over OS detection while that app is frontmost. The socket also works the other way: Hitch sends a command to the Zellij plugin, which writes it into the last focused pane with Zellij's write-chars API. If the plugin isn't loaded, Hitch falls back to the CLI, `zellij --session <name> action write-chars`. Zed's extension API is narrower than VS Code's, so the spike must confirm a Zed extension can see the active file; if not, Hitch falls back to Zed's window title, which shows the project and file.

**clawswap.** clawswap already knows each project's source folder, and HITCH.md lives there. When the browser is frontmost, Hitch asks clawswap's daemon on 127.0.0.1:27390 for the project and uses its `root`:

| Endpoint | Exists today? | What it does |
|---|---|---|
| `GET /projects` | Yes | Each project's `root`, its source folder |
| `GET /p/<id>/events` | Yes | Branch switches, so the panel can show the current branch |
| `POST /p/<id>/focus` | New | Called by a project's page when its window gains focus; records it as the focused project |
| `GET /focus/events` | New | Server-Sent Events for Hitch: `{"project", "root", "at"}` each time the focused project changes |

clawswap must report which project's page has focus. The page calls `POST /p/<id>/focus` from clawswap's embedded script, automatically, so apps already embedding clawswap need no change. clawswap's existing origin check means only that project's own dev servers can claim focus for it. When the browser is frontmost, Hitch uses the latest focused project from `/focus/events`.

No browser extension is needed.

Wayland is the main risk on Ubuntu: by design it hides other apps' windows, so Hitch needs a small GNOME Shell extension to learn the focused window. The spike should prove this early. These mechanisms are from memory and need checking against current GNOME and Ubuntu releases.

File path detection varies by app, so the spike should test the apps you use most.

## Privacy and security

Hitch watches what the user is doing, so trust is the product: nothing leaves the machine, and nothing about activity is stored except the notes the user writes.

- No network access in v1 apart from an opt-in update check.
- No activity history. The current context is held in memory only and dropped on the next switch.
- Window titles and URLs are read only to match rules; they are never logged.
- The ignore list is applied before matching, and private browser windows are always ignored.
- Permissions are requested only when needed, with a plain explanation of each.
- Signed and notarised builds on macOS, signed .deb packages with published checksums on Ubuntu.

## Competitive landscape

Ghostnote is the closest product; Hitch's edge is cross-platform support, folder and URL contexts, and plain Markdown storage. Details below are from memory and listing pages, not hands-on testing.

| Product | What it does | Gap Hitch fills |
|---|---|---|
| [Ghostnote](https://alternativeto.net/software/ghostnote) | Notes per app or document, Mac only | Linux, project folders, URLs, plain files |
| [SideNotes](https://apps.apple.com/app/sidenotes/id1441958036) | Notes panel on the screen edge, Mac only | Notes change with context instead of one fixed list |
| [Contextly Notes](https://apps.apple.com/us/app/contextly-notes/id6762497168) | Context-based notes, Apple platforms | To be checked hands-on |
| Obsidian, Apple Notes, OneNote | General notes | Hitch surfaces notes; these need searching |

## Milestones and open questions

Build in four steps, each usable on its own; no dates are set yet.

1. **Spike:** prove app and window detection on the latest Ubuntu through the GNOME extension, prototype the Zed, Zellij and Clawswap integrations, and measure switch latency.
2. **MVP (Ubuntu):** panel, quick capture, copy and send to Zellij, HITCH.md files, app and folder contexts, search, .deb and AppImage packages.
3. **macOS:** detection on macOS, the Zed and Zellij plugins tested there, signed and notarised build.
4. **Polish:** URL and window-title contexts, show on arrival, import and export, public beta.

**Open questions**

- Which clawswap version ships the focus endpoints and the embed-script change?
- Should the panel show the clawswap branch next to the project name?
- Which open source license: MIT, Apache-2.0, or both?
- iced or GTK4 for the native UI (see Architecture)?
- Check hitch-related domains and trademarks before going public.

**Decided:** Ubuntu first (latest release), then macOS; HITCH.md is committed with the project; the app will probably be open source; the shell is native Rust; file contexts are the main ones; clawswap gives Hitch the project's source folder, where HITCH.md lives, and its embedded script reports which project's page has focus.
