//! Where a password comes from when nobody types it at a prompt (ledger 638).
//!
//! Split out of `login.rs` when the file neared its size ceiling. The seam is
//! `--password-stdin` itself: everything a terminal is needed for lives here,
//! so the DECISIONS underneath it — is stdin usable, was a real password
//! read — stay exercisable without one.

use std::io::{self, BufRead as _, IsTerminal as _};

use super::LoginError;

/// Whether `--password-stdin` may proceed, decided from a caller-supplied
/// terminal flag rather than a live stdin.
///
/// PURE, for the reason every other CLI-argument rule in this crate is: a
/// terminal is not something a unit test can attach to stdin.
///
/// **REFUSED, and deliberately unlike `docker login`.** Docker reads a
/// password typed at a real terminal through this same flag without
/// complaint (`verifyLoginOptions` in `cli/command/registry/login.go` runs
/// `io.ReadAll` on stdin with no terminal check at all). `rpassword` is used
/// in `login.rs` so a password is never echoed and never reaches scrollback;
/// a plain `read_line` off an interactive terminal does neither of those —
/// every keystroke lands on the screen in plain sight, which is the exact
/// leak `--password-stdin` exists to route around. A pipe and a redirected
/// file both report `false` here, and both are the ordinary case for
/// automation.
pub(super) fn stdin_password_allowed(is_terminal: bool) -> Result<(), LoginError> {
    if is_terminal {
        Err(LoginError::StdinIsTerminal)
    } else {
        Ok(())
    }
}

/// Strip the one line ending `--password-stdin` promises to strip, and refuse
/// an empty result.
///
/// PURE, so the edge cases are exercisable without a pipe: a password that IS
/// a lone newline, one that never got a line ending because the pipe closed
/// early. **ONLY a trailing `\n` or `\r\n` is removed — this is not `trim`.**
/// A password can legitimately start, end, or consist of whitespace, and
/// `trim` would silently hand the gateway a password nobody typed.
pub(super) fn stdin_password(line: &str) -> Result<String, LoginError> {
    // THE `\r` IS ONLY EVER PART OF THE LINE ENDING, never stripped alone.
    // `BufRead::read_line` stops at `\n` and nothing else, so a trailing `\r`
    // with no `\n` after it is not a line ending that arrived early — it is
    // one character of a password on a platform that never sent a newline at
    // all, and stripping it regardless would silently shorten that password.
    let stripped = match line.strip_suffix('\n') {
        Some(rest) => rest.strip_suffix('\r').unwrap_or(rest),
        None => line,
    };
    if stripped.is_empty() {
        Err(LoginError::EmptyPasswordStdin)
    } else {
        Ok(stripped.to_string())
    }
}

/// Refuse `--password-stdin` outright when stdin is the real terminal.
///
/// The only impure half of this module: everything it decides is
/// [`stdin_password_allowed`], read here off a live `stdin` rather than a
/// caller-supplied flag.
pub(super) fn ensure_stdin_pipeable() -> Result<(), LoginError> {
    stdin_password_allowed(io::stdin().is_terminal())
}

/// Read the password `--password-stdin` promises: the first line of stdin,
/// its line ending stripped, refused if empty.
///
/// **THE FIRST LINE ONLY**, read with one `read_line` rather than draining
/// stdin to EOF — a caller piping the password from a file is not obliged to
/// end the stream there, and nothing else in this binary assumes stdin closes
/// after one value either.
pub(super) fn read_password_stdin() -> Result<String, LoginError> {
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    stdin_password(&line)
}
