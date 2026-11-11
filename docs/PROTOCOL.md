# Hitch protocols

Hitch learns context from three kinds of sources, each with its own channel:

| Source | Channel | Direction |
|---|---|---|
| Zed extension, Zellij plugin, `hitch` CLI | Hitch socket (this document) | both ways |
| GNOME Shell extension | D-Bus, session bus | Hitch subscribes |
| clawswap | HTTP + SSE on 127.0.0.1:27390 | Hitch subscribes |

The Rust types for the socket are in [`crates/hitch-protocol`](../crates/hitch-protocol). When this document and the crate disagree, fix one of them in the same PR.

## The Hitch socket

- **Transport:** Unix domain stream socket at `~/.hitch/hitch.sock`. `$HITCH_SOCKET` overrides the path (tests, several instances). Hitch creates `~/.hitch` with mode 0700 and the socket with mode 0600, so only the user can connect.
- **Framing:** one JSON object per line, UTF-8, terminated by `\n`. A line is at most 64 KiB; longer lines may be dropped and the connection closed.
- **Messages:** every object has a `"type"`. Receivers ignore unknown types and unknown fields. Adding a type or an optional field is not a breaking change; anything else bumps `protocol` (currently `1`).
- **Connections:** a plugin keeps one connection open, sends `hello` first, and reconnects with backoff (start at 1 s, cap at 30 s) if Hitch isn't running or restarts. One-shot clients such as the CLI may skip `hello`, send their messages and close.

### Client to Hitch

**`hello`**, first on a long-lived connection:

```json
{"type":"hello","client":"hitch-zellij","version":"0.1.0","protocol":1,"capabilities":["write_chars"]}
```

`capabilities` lists the Hitch-to-client messages the client can act on.

**`context`**, whenever the source's context changes while it is focused, and once when it gains focus:

```json
{"type":"context","source":"zed","project_root":"/home/u/code/hitch","file":"/home/u/code/hitch/src/main.rs"}
{"type":"context","source":"zellij","cwd":"/home/u/code/hitch","session":"hitch","pane":"3"}
```

| Field | Type | Meaning |
|---|---|---|
| `source` | string | `zed`, `zellij`, `clawswap`, `gnome_shell`, `macos`, `cli`, or any other name |
| `project_root` | path, optional | Project root as the source sees it (Zed worktree) |
| `file` | path, optional | Absolute path of the active file |
| `cwd` | path, optional | Working folder (a Zellij pane's cwd) |
| `url` | string, optional | Page URL, for sources that know one |
| `session` | string, optional | Source session name (the Zellij session) |
| `pane` | string, optional | Source pane or window id (the Zellij pane id) |

Paths are absolute. Omit a field rather than sending `null` or `""`. A plugin report is exact, so it wins over OS detection while that app is frontmost.

**`blur`**, when the source loses focus or is closing:

```json
{"type":"blur","source":"zed"}
```

Hitch stops preferring that source's last report but keeps its notes on screen until another file context arrives (spec, matching rule 3).

**`command`**, from the CLI or a shortcut:

```json
{"type":"command","command":"toggle_panel"}
```

Commands: `toggle_panel`, `show_panel`, `hide_panel`, `capture`, `toggle_pin`, `search`. `hitch toggle`, `hitch show`, `hitch hide`, `hitch capture`, `hitch pin` and `hitch search` send them.

### Hitch to client

**`welcome`**, reply to `hello`:

```json
{"type":"welcome","protocol":1,"version":"0.1.0"}
```

**`write_chars`**, sent only to clients with the `write_chars` capability (the Zellij plugin). Type `text` into the last focused pane; with `submit`, also press Enter.

```json
{"type":"write_chars","text":"cargo run","submit":false}
```

**`error`**, the last message could not be handled:

```json
{"type":"error","message":"unsupported protocol 2"}
```

### Sandboxed plugins

Zed extensions and Zellij plugins run as wasm and may not be able to open a Unix socket. They can use the CLI instead:

- **Plugin to Hitch:** run `hitch send '<json>'` as a process, one argument per message (or one message per line of stdin). It sends them on a one-shot connection.
- **Hitch to the Zellij plugin:** Hitch delivers the same Hitch-to-client JSON through `zellij --session <session> pipe --name hitch -- '<json>'`. If no plugin is loaded, Hitch falls back to `zellij --session <session> action write-chars`.

The integration threads should confirm which of these each plugin needs and update this section.

## GNOME Shell extension (D-Bus)

On Wayland only the shell knows the focused window, so the extension in [`integrations/gnome-shell`](../integrations/gnome-shell) exports it on the session bus. Hitch subscribes when it starts and calls `GetFocus` for the initial state; the extension needs no reconnect logic. Its README has the full introspection XML.

- Bus name `io.github.cbishopvelti.Hitch.Shell`, object path `/io/github/cbishopvelti/Hitch/Shell`, interface `io.github.cbishopvelti.Hitch.Shell`.
- **Method `GetFocus() -> (a{sv} focus)`**.
- **Signal `FocusChanged(a{sv} focus)`**, when the focused window changes and when its title changes.
- **Method `SetKeepAbove(b pinned)`**: keeps every window of the calling process (matched by the caller's PID) above others and on all workspaces, including windows it opens later, until called with `false` or the caller leaves the bus. This is how the panel pin works on Wayland.
- **Property `Version` (u, read-only)**, currently `1`.

`focus` keys, always present; all empty or 0 when nothing is focused (desktop, overview):

| Key | Type | Meaning |
|---|---|---|
| `app_id` | s | Desktop file id without `.desktop`, e.g. `dev.zed.Zed`; `""` if unknown |
| `wm_class` | s | Wayland app_id or X11 WM_CLASS |
| `title` | s | Window title |
| `pid` | u | Process id, 0 if unknown |
| `sandboxed_app_id` | s | Flatpak or Snap id, or `""` |

Hitch ignores `FocusChanged` where `pid` is its own (the panel itself got focus). Hitch's Wayland app id is `io.github.cbishopvelti.Hitch` (`hitch_protocol::APP_ID`). On the Rust side, use `zbus`.

## clawswap (HTTP)

See the spec, "Architecture and platform", for the endpoints. Hitch reads `GET /projects` for each project's `root` and subscribes to `GET /focus/events` (Server-Sent Events, `{"project", "root", "at"}`) while the browser is frontmost, turning each event into a `context` with `source: "clawswap"` and `project_root: root`.
