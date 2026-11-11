# Hitch plugin for Zellij

Reports the focused pane's working folder to Hitch (spec F2), and types commands Hitch sends into the last focused pane (spec F13). Not built yet.

- Speaks the Hitch socket protocol in [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md): `hello` with capability `write_chars`; `context` with `source: "zellij"`, `cwd`, `session` and `pane`; handles `write_chars` with Zellij's write-chars API (Enter only when `submit` is true).
- Zellij plugins are wasm, so this is a separate Cargo project (not a workspace member). Depend on the shared types with `hitch-protocol = { path = "../../crates/hitch-protocol" }`; it builds for `wasm32-wasip1`.
- Plugins can't open sockets: report with `run_command(["hitch", "send", json])` and receive Hitch's messages through `zellij pipe --name hitch`. Confirm and update the protocol doc.
