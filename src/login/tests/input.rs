//! What `--password-stdin` decides before a single byte is read (ledger 638).
//!
//! Split out the same way `reconcile.rs` was: the seam is the module beside
//! it, and this file is only about the pure decisions in `login::input`.

use super::super::input::{stdin_password, stdin_password_allowed};
use super::super::LoginError;

#[test]
fn a_terminal_is_refused_before_anything_is_read() {
    // THE WHOLE POINT of refusing rather than following `docker login`: a
    // real terminal here would echo every keystroke in plain sight.
    assert!(matches!(
        stdin_password_allowed(true),
        Err(LoginError::StdinIsTerminal)
    ));
}

#[test]
fn a_pipe_or_a_redirected_file_is_allowed() {
    assert!(stdin_password_allowed(false).is_ok());
}

#[test]
fn a_trailing_newline_is_stripped() {
    assert_eq!(stdin_password("hunter2\n").unwrap(), "hunter2");
}

#[test]
fn a_trailing_crlf_is_stripped() {
    // A password piped from a file saved on Windows, or through a tool that
    // writes CRLF line endings, must not carry a `\r` into the credential.
    assert_eq!(stdin_password("hunter2\r\n").unwrap(), "hunter2");
}

#[test]
fn a_password_with_no_line_ending_is_kept_whole() {
    // stdin can close before a newline arrives — the line that DID arrive is
    // still the password, not a truncated one.
    assert_eq!(stdin_password("hunter2").unwrap(), "hunter2");
}

#[test]
fn leading_and_interior_whitespace_survive() {
    // NOT `trim`. A password can legitimately start, end, or consist of
    // whitespace, and trimming would silently hand the gateway a password
    // nobody typed.
    assert_eq!(stdin_password(" hunter 2 \n").unwrap(), " hunter 2 ");
}

#[test]
fn an_empty_line_is_refused_rather_than_treated_as_a_blank_password() {
    assert!(matches!(
        stdin_password("\n"),
        Err(LoginError::EmptyPasswordStdin)
    ));
    assert!(matches!(
        stdin_password(""),
        Err(LoginError::EmptyPasswordStdin)
    ));
}

#[test]
fn a_lone_carriage_return_is_not_mistaken_for_an_empty_line() {
    // A bare `\r` with no `\n` is one character of password on a platform
    // that never sent a line ending at all, not a blank line.
    assert_eq!(stdin_password("\r").unwrap(), "\r");
}
