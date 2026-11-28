//! What BariStore asks of rootshell's terminal (Ghostty) by printing it: a
//! WebAssembly program can run no command, so `openurl` and notifications go
//! as escape sequences on standard output.
//!
//! * OSC 1337 `OpenURL` (iTerm2's; rootshell docs/remote-url-opening.md,
//!   TerminalView+SessionHost.swift `openProgramURL`): rootshell opens an
//!   http(s) link with iOS, in Safari, when Settings › Terminal › Open Links
//!   from Programs is on, rootshell is in front and the tab is shown; one a
//!   second at most. Other schemes are refused.
//! * OSC 777 `notify` (Ghostty's desktop notification): rootshell posts it,
//!   in the background too, when Settings › Notifications › Terminal
//!   Notifications is on (GhosttyApp.swift DESKTOP_NOTIFICATION).
//! * OSC 8: a link the terminal shows underlined; a long press opens it.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::io::Write;

/// A line on the terminal. Never panics (the tab may be gone).
pub fn say(text: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(format!("{text}\n").as_bytes());
    let _ = out.flush();
}

/// Text that cannot end or split a sequence: no control characters, and
/// none of the separators the notification's fields are split at.
fn plain(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).map(|c| if c == ';' { ',' } else { c }).collect()
}

/// Asks rootshell to open `url` (http or https) in Safari.
pub fn open_url(url: &str) {
    say(&format!("\x1b]1337;OpenURL=:{}\x07", B64.encode(url)));
}

/// A notification with `title` and `body`.
pub fn notify(title: &str, body: &str) {
    say(&format!("\x1b]777;notify;{};{}\x07", plain(title), plain(body)));
}

/// `text`, as a link to `url`.
pub fn link(url: &str, text: &str) -> String {
    format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", plain(url), plain(text))
}

#[cfg(test)]
mod tests {
    #[test]
    fn sequences() {
        assert_eq!(super::plain("a;b\x07c\x1bd"), "a,bcd");
        assert_eq!(super::link("http://x/", "x"), "\x1b]8;;http://x/\x1b\\x\x1b]8;;\x1b\\");
    }
}
