# Hitch extension for Zed

Reports the active file and project root to Hitch (spec F2). Not built yet.

- Speaks the Hitch socket protocol in [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md): `context` with `source: "zed"`, `project_root` and `file`; `blur` when Zed loses focus.
- Zed extensions are wasm, so this is a separate Cargo project (not a workspace member). Depend on the shared types with `hitch-protocol = { path = "../../crates/hitch-protocol" }`; it builds for `wasm32-wasip1`.
- If the extension can't open a socket, run `hitch send '<json>'` instead.
- First check whether Zed's extension API can see the active file. If it can't, Hitch falls back to Zed's window title, which shows the project and file; record the finding here.
