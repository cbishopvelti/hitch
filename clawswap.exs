# clawswap config for developing Hitch: `clawswap up` in this repo opens a session with the panel running,
# and `clawswap switch <branch>` stops it, checks the branch out, rebuilds what changed and starts it again.
# https://cbishopvelti.github.io/clawswap/configuration.html
[
  name: "hitch",
  servers: [
    # Rebuilds whatever a switch changed before it starts the panel; hitch-web is the window "Branches" opens.
    # Hitch listens on a Unix socket, not a TCP port, so there's no port for a switch to wait on.
    hitch: [
      command: "cargo build -p hitch-web && cargo run -p hitch-app",
      match: ~r/cargo (build|run) -p hitch|target\/debug\/hitch/
    ]
  ],
  panes: [
    # An empty shell at the repo root, for `cargo test --workspace`, `hitch toggle` and the like.
    shell: []
  ],
  # Each runs on the first `clawswap up`, then after a switch only when its files changed.
  steps: [
    [
      name: "gnome",
      label: "Installing the GNOME extension",
      when: "integrations/gnome-shell/hitch@cbishopvelti.github.io/",
      run: "make -C integrations/gnome-shell install"
    ],
    [
      name: "zellij",
      label: "Building the Zellij plugin",
      when: ["integrations/zellij/", "crates/hitch-protocol/"],
      run:
        "rustup target add wasm32-wasip1 && cargo build --release --target wasm32-wasip1 && " <>
          "mkdir -p ~/.local/share/zellij/plugins && " <>
          "cp target/wasm32-wasip1/release/hitch-zellij.wasm ~/.local/share/zellij/plugins/",
      cwd: "integrations/zellij"
    ]
  ],
  notes: [
    [
      when: "integrations/gnome-shell/hitch@cbishopvelti.github.io/",
      message: "The GNOME extension changed: log out and back in to load it"
    ],
    [
      when: ["integrations/zellij/", "crates/hitch-protocol/"],
      message: "The Zellij plugin was rebuilt: start a new zellij session to load it"
    ],
    [when: "integrations/zed/", message: "The Zed settings changed: see integrations/zed/README.md"]
  ]
]
