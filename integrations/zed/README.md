# Hitch and Zed

Hitch shows the notes for the file you have open in Zed (spec F2). There is no
Zed extension to install: one line in Zed's settings is enough.

## Setup

Zed 1.20 or later. Add this to `~/.config/zed/settings.json` (Zed: _Open
Settings File_):

```jsonc
{
  "window_title_format": "${projectName}${separator}${filePath}"
}
```

Zed's window title then reads `hitch — /home/u/code/hitch/src/main.rs`, and
changes whenever you switch tabs. With Zed's default client-side decorations
nothing looks different inside Zed, since Zed draws its own title bar; the
title shows in the overview and Alt+Tab.

Without the setting, Zed's title is `hitch — main.rs`. Hitch can't tell which
`main.rs` that is, so it falls back to the app context and should suggest the
setting.

## How it works

1. The GNOME Shell extension sends `FocusChanged` on D-Bus whenever the focused
   window or its title changes (`docs/PROTOCOL.md`). For Zed, `app_id` is
   `dev.zed.Zed` (or `-Preview`, `-Nightly`, `-Dev`).
2. Hitch's context engine checks the app id with
   `hitch_protocol::zed::is_zed_app_id` and parses the title with
   `hitch_protocol::zed::parse_window_title`.
3. A title with a path gives `ZedTitle::File`, which Hitch treats like a
   `context` report from Zed (`ZedTitle::report()`): the active file, resolved
   to its project by the usual matching rules.
4. When focus moves to another app, that is Zed's `blur`.

Project names are folder names and can't contain `/`, so the parser takes the
first `/` as the start of the path. That works with any
`window_title_separator`, and with paths that contain spaces or `—`. The ` ↗`
and ` ↙` markers Zed adds for shared projects are dropped.

On macOS (later), the same title is available through the Accessibility API.
Zed also sets the window's represented file there (`AXDocument`), which gives
the path without any setting.

## Why not a Zed extension

Checked against `zed_extension_api` 0.7.0 (the latest release, November 2026)
and Zed's `main` branch. Extensions are wasm components that only answer calls
from Zed: language server commands and options, completion and symbol labels,
slash commands, context servers, docs indexing and debug adapters. Nothing in
the API reports the active editor, the active file, tab switches or window
focus, and an extension has no event loop of its own.

The closest workaround is an extension that starts a do-nothing language
server, which hears `textDocument/didOpen` and `didChange`. We decided against
it:

- LSP has no "this tab is now active" message. Switching between open tabs
  sends nothing.
- `didOpen` also fires for buffers you aren't looking at: restored tabs at
  startup, project search, go to definition, multibuffers, rename across files.
  A plugin report wins over OS detection, so these would show the wrong notes.
- It needs a native binary shipped and found on `PATH`, and a hand-kept list of
  every language Zed knows, since a language server can't be registered for all
  languages.

The window title is exact, updates on every tab switch, and costs one setting.
If Zed's extension API gains an active-editor or workspace event, a real
extension can send `context` and `blur` over the socket like the Zellij plugin,
and this README should be updated.

## Not verified yet

These need a real Ubuntu desktop with Zed and the GNOME extension:

- that Mutter passes Zed's Wayland title through to `Meta.Window.title` on
  every tab switch, so `FocusChanged` fires (Zed sets it with
  `xdg_toplevel.set_title`);
- the title of a remote (SSH) project: `${filePath}` is then a path on the
  remote machine, so Hitch should ignore paths that don't exist locally;
- the Flatpak build of Zed, whose `app_id` should still be `dev.zed.Zed`.

To check by hand on GNOME, with the Hitch Shell extension enabled and Zed
focused after a short delay:

```sh
sleep 3; gdbus call --session --dest io.github.cbishopvelti.Hitch.Shell \
  --object-path /io/github/cbishopvelti/Hitch/Shell \
  --method io.github.cbishopvelti.Hitch.Shell.GetFocus
```

The `title` should end with the path of the file in the active tab.
