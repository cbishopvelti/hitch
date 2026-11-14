# Hitch

Notes that follow your work. Hitch is a native Rust desktop app that shows the notes for whatever you are working on right now (a file, a project folder, a page) and lets you add one in a keystroke. Notes live in a `HITCH.md` file in the project they belong to.

Ubuntu (latest, Wayland) first, then macOS. Early development: nothing works yet.

- [Product spec](docs/SPEC.md)
- [Socket and D-Bus protocols](docs/PROTOCOL.md)

## Layout

| Path | What it is |
|---|---|
| `crates/hitch-protocol` | Shared context types and the socket protocol. No deps beyond serde; builds for wasm so plugins can use it |
| `crates/hitch-store` | HITCH.md parsing and writing, project folder resolution, fallback files, SQLite index |
| `crates/hitch-context` | Context engine: GNOME D-Bus focus, macOS focus, socket server, clawswap client, resolver, debounce |
| `crates/hitch-macos` | macOS glue: frontmost app (NSWorkspace), window title and file (Accessibility API), pin on every Space, global shortcuts. The only crate with `unsafe` |
| `crates/hitch-web` | `hitch-web`, the small web view window the panel's "Branches" button opens for clawswap's branch page (wry: WebKitGTK on Linux, WKWebView on macOS) |
| `crates/hitch-app` | The `hitch` binary: iced panel, quick capture, and the CLI (`hitch toggle`, `hitch send`, ...) |
| `integrations/gnome-shell` | GNOME Shell extension that reports the focused window over D-Bus and pins the panel |
| `integrations/zed` | Zed extension (wasm, separate Cargo project) |
| `integrations/zellij` | Zellij plugin (wasm, separate Cargo project) |

Dependencies point one way: `hitch-app` → `hitch-context` → `hitch-store` → `hitch-protocol`, and on macOS `hitch-context` → `hitch-macos`. Keep each change inside the crate or directory that owns it; a change to `hitch-protocol` or `docs/PROTOCOL.md` affects every plugin, so make it in its own PR.

## Build

```sh
cargo build
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p hitch-app        # starts the panel
```

On Linux, Hitch writes its desktop entry and icon ([`data/`](data/)) to `~/.local/share/applications` and `~/.local/share/icons` when it starts, pointing at the binary that ran. GNOME uses them for the panel's icon in Alt+Tab, the dock and the app grid.

### With clawswap

[`clawswap.exs`](clawswap.exs) sets the repo up for [clawswap](https://github.com/cbishopvelti/clawswap): `clawswap up` opens a zellij (or tmux) session with the panel running (`cargo run -p hitch-app`) and a shell, and `clawswap switch <branch>` stops the panel, checks the branch out and starts it again. The first `clawswap up`, and any switch that changes them, also installs the GNOME extension (`make -C integrations/gnome-shell install`) and builds the Zellij plugin into `~/.local/share/zellij/plugins/`; clawswap then says what needs a restart to load them.

While clawswap runs the project you're working in, the panel shows **Branches**. It opens that project's branch page (`http://127.0.0.1:27390/p/<id>`, with its Switch and Pull buttons) in `hitch-web`, a small window beside the panel, kept on top while the panel is pinned. Links off the page open in your browser. Without `hitch-web` next to `hitch` or on your PATH, the page opens in the browser instead.

Rust 1.88 or newer. On Ubuntu, `hitch-web` needs WebKitGTK: `sudo apt install libwebkit2gtk-4.1-dev`. CI runs the same checks on Ubuntu and macOS and also builds `hitch-protocol` for `wasm32-wasip1`.

## macOS

`packaging/macos/build-app.sh` builds `target/release/Hitch.app` (CI uploads it as the `Hitch-macos-arm64` artifact). It is signed ad hoc, not notarised, so the first time macOS refuses to open it: allow it under System Settings > Privacy & Security, or run `xattr -dr com.apple.quarantine Hitch.app`.

- **Accessibility permission.** Window titles and open files need it; without it Hitch only knows the frontmost app. Hitch asks on first start. Switch Hitch on under System Settings > Privacy & Security > Accessibility. Run from a terminal (`cargo run -p hitch-app`), the permission belongs to the terminal app instead. An ad hoc signed build counts as a new app each time, so switch it off and on again after rebuilding.
- **Shortcuts.** Cmd+Shift+N shows or hides the panel and Cmd+Shift+H opens quick capture from any app. They take over those keys in other apps (Finder's New Folder, a browser's private window).
- **Pin.** A pinned panel floats over other windows and appears on every Space, including over full-screen apps.
- **Zed.** Hitch reads the open file from the window's represented file (`AXDocument`) where Zed sets it, so the `window_title_format` setting in `integrations/zed` should not be needed on a Mac; it still works as a fallback.
- **CLI.** For `hitch toggle` and for the Zellij plugin's `hitch send`, put the binary on your PATH: `ln -s /Applications/Hitch.app/Contents/MacOS/hitch /usr/local/bin/hitch`.

## License

Not chosen yet (MIT, Apache-2.0 or both is an open question in the spec).
