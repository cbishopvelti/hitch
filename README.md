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
| `crates/hitch-context` | Context engine: GNOME D-Bus focus, socket server, clawswap client, resolver, debounce |
| `crates/hitch-app` | The `hitch` binary: iced panel, quick capture, and the CLI (`hitch toggle`, `hitch send`, ...) |
| `integrations/gnome-shell` | GNOME Shell extension that reports the focused window over D-Bus and pins the panel |
| `integrations/zed` | Zed extension (wasm, separate Cargo project) |
| `integrations/zellij` | Zellij plugin (wasm, separate Cargo project) |

Dependencies point one way: `hitch-app` → `hitch-context` → `hitch-store` → `hitch-protocol`. Keep each change inside the crate or directory that owns it; a change to `hitch-protocol` or `docs/PROTOCOL.md` affects every plugin, so make it in its own PR.

## Build

```sh
cargo build
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p hitch-app        # starts the panel
```

Rust 1.88 or newer. CI runs the same checks on Ubuntu and also builds `hitch-protocol` for `wasm32-wasip1`.

## License

Not chosen yet (MIT, Apache-2.0 or both is an open question in the spec).
