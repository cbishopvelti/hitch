# Hitch plugin for Zellij

A background Zellij plugin with two jobs:

- **Context (spec F2).** It reports the focused terminal pane's working folder to Hitch, so the panel shows the notes of the project that contains that folder. It reports when you switch pane or tab, and when the shell in the focused pane changes folder.
- **Send to Zellij (spec F13).** It types commands Hitch sends into the last focused terminal pane, left unrun so you can check them. The run-immediately variant (Shift+click in Hitch) also presses Enter. A multi-line command is sent as a bracketed paste, so shells that support it (bash 5.1+, zsh, fish) don't run any line until you press Enter.

It speaks the protocol in [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md). Zellij plugins can't open the Hitch socket, so it reports by running `hitch send '<hello>' '<context>'`, and Hitch reaches it with `zellij --session <name> pipe --name hitch -- '<json>'`.

Needs Zellij 0.46 or later.

## Build

This is its own Cargo project (not a member of the root workspace) and builds for wasm:

```sh
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
# -> target/wasm32-wasip1/release/hitch-zellij.wasm
```

Tests run on the host:

```sh
cargo test --target x86_64-unknown-linux-gnu
```

## Install

Copy the wasm somewhere stable and load it once per session in `~/.config/zellij/config.kdl`:

```kdl
load_plugins {
    "file:~/.local/share/zellij/plugins/hitch-zellij.wasm" {
        // Optional: the hitch binary, if it isn't on Zellij's PATH.
        // hitch_path "/home/you/.cargo/bin/hitch"
    }
}
```

The plugin asks for four permissions: read application state (panes, tabs, folders), run commands (`hitch send`), write to panes (typing commands), and read CLI pipes (messages from Hitch). A background plugin has no pane to show the prompt in, so grant them once by opening it in a pane:

```sh
zellij plugin -- file:~/.local/share/zellij/plugins/hitch-zellij.wasm
```

Press `y`. Zellij remembers the grant in `~/.cache/zellij/permissions.kdl`. Opened in a pane, the plugin also shows its status: whether it can reach Hitch and which pane commands go to.

## Try it by hand

```sh
# What the plugin reports (with a fake `hitch` on PATH that logs its arguments):
#   {"type":"hello","client":"hitch-zellij","version":"0.1.0","protocol":1,"capabilities":["write_chars"]}
#   {"type":"context","source":"zellij","cwd":"/home/you/code/hitch","session":"hitch","pane":"0"}

# Type a command into the last focused pane, unrun:
zellij --session hitch pipe --name hitch -- '{"type":"write_chars","text":"cargo test","submit":false}'
# And run it:
zellij --session hitch pipe --name hitch -- '{"type":"write_chars","text":"cargo test","submit":true}'
```

The pipe exits 0 when the text was typed, and 1 with a one-line reason when it wasn't.

## Notes

- Commands go to the last focused *terminal* pane. Focusing a plugin pane (such as the file browser) doesn't change the target.
- If Hitch isn't running, the plugin retries its newest report with backoff, from 1 s up to 30 s, and stops once Hitch accepts it.
- Zellij doesn't know when its terminal window loses OS focus, so the plugin can't send `blur` then. Hitch relies on window detection for that. The plugin sends `blur` when Zellij tells it the session is closing.
- Load the plugin once per session. A pipe reaches every instance, so a second instance would type each command twice.
