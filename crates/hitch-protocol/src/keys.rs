//! The keystrokes typed into a Zellij pane for a `write_chars` message.
//!
//! Shared by the Zellij plugin (`write_to_pane_id`) and Hitch's fallback,
//! `zellij action write-chars`, so both type the same thing.

/// Start and end of a bracketed paste (xterm). Shells that support it (bash
/// 5.1+, zsh, fish) insert pasted text without running any of its lines.
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// The keystrokes to type into the pane for `text`.
///
/// - Trailing newlines are dropped, so a code block copied with its final
///   newline is left unrun unless `submit` is set.
/// - Text with several lines is sent as a bracketed paste, so the shell
///   doesn't run each line as it arrives.
/// - With `submit`, Enter (`\r`) is pressed at the end.
pub fn keystrokes(text: &str, submit: bool) -> String {
    let text = text.trim_end_matches(['\r', '\n']);
    let mut out = String::with_capacity(text.len() + PASTE_START.len() + PASTE_END.len() + 1);
    if text.contains('\n') {
        out.push_str(PASTE_START);
        // Terminals send Enter as \r; inside a paste, newlines are \n but a
        // stray \r\n from Windows line endings would double up.
        out.push_str(&text.replace("\r\n", "\n"));
        out.push_str(PASTE_END);
    } else {
        out.push_str(text);
    }
    if submit {
        out.push('\r');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_is_typed_unrun() {
        assert_eq!(keystrokes("cargo run", false), "cargo run");
        assert_eq!(keystrokes("cargo run\n", false), "cargo run");
    }

    #[test]
    fn submit_presses_enter() {
        assert_eq!(keystrokes("cargo run\n", true), "cargo run\r");
    }

    #[test]
    fn several_lines_are_pasted() {
        assert_eq!(
            keystrokes("cd x\r\nmake\n", false),
            "\x1b[200~cd x\nmake\x1b[201~"
        );
        assert_eq!(keystrokes("a\nb", true), "\x1b[200~a\nb\x1b[201~\r");
    }

    #[test]
    fn empty_text_with_submit_is_just_enter() {
        assert_eq!(keystrokes("", true), "\r");
        assert_eq!(keystrokes("", false), "");
    }
}
