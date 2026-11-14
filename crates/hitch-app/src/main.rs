//! `hitch`: the desktop app and its command line.
//!
//! With no arguments it starts the panel. With a command it sends that
//! command to the running app over the socket and exits:
//!
//! ```text
//! hitch                 start Hitch
//! hitch toggle          show or hide the panel
//! hitch show | hide
//! hitch capture         quick capture for the current context
//! hitch pin             pin or unpin the panel
//! hitch search
//! hitch send [JSON...]  forward JSON messages (arguments, or stdin lines)
//! ```
//!
//! `hitch send` exists for plugins that run in a sandbox without socket
//! access (Zed and Zellij wasm plugins): they run `hitch send '<json>'` as a
//! process instead.

mod cli;
#[cfg(target_os = "linux")]
mod desktop;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Invocation::RunApp) => match ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("hitch: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Invocation::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(inv) => match cli::execute(inv) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("hitch: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("hitch: {e}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}
