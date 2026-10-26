//! The check for writes before gh as main shipped it (7e27ca4), kept as a
//! floor under the substitution reading in `bin_dirs.rs` (ADR-0645: a gate
//! change may not accept an input the shipped gate refuses).
//!
//! An unquoted `$(…)` ends the command at `$`; on main the first word of
//! EVERY later command is read as the rest of that argument, starting with
//! the substitution's output. That is sticky: the substitution's own
//! commands come between, and the floor does not count parens. Its one
//! known false refusal is the reason this floor has an exception: the lone
//! `}` closing a group after `b=$(x);` read as `$()}`.

use super::{executed, reader_arguments, Target, CLOSE_BRACE};
use crate::install::shellword::SimpleCommand;

/// Main's state across a line: after a word ending in `$`, whether the
/// argument a substitution started is read.
#[derive(Default)]
pub(super) struct Shipped {
    continues: Option<bool>,
}

impl Shipped {
    /// Main's answer for *c*. With *lone_brace*, a command that is only `}`
    /// is not read as continuing a substitution.
    pub(super) fn judged(&mut self, c: &SimpleCommand, to: &Target, lone_brace: bool) -> bool {
        let program = executed(c).map(|(_, args)| c.words.len() - args.len() - 1);
        let reader = reader_arguments(c);
        let carried = self.continues;
        let brace = lone_brace && c.words.len() == 1 && c.words[0] == CLOSE_BRACE.to_string();
        let hit = c
            .words
            .iter()
            .chain(&c.writes_to)
            .enumerate()
            .any(|(n, word)| {
                let is_word = n < c.words.len();
                if n == 0 && is_word {
                    if let Some(read) = carried {
                        return !read && !brace && to.may_hold(&format!("$(){word}"));
                    }
                }
                if is_word && (program == Some(n) || reader.is_some_and(|start| n >= start)) {
                    return false;
                }
                let value = word.split_once('=').map(|(_, v)| v);
                std::iter::once(word.as_str())
                    .chain(value)
                    .any(|w| to.may_hold(w))
            });
        if c.words.last().is_some_and(|w| w.ends_with('$')) {
            self.continues = Some(carried.unwrap_or(false) || reader.is_some());
        }
        hit
    }
}
