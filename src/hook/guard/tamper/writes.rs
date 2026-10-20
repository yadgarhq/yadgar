//! Names a line may write in a way the expander does not model.
//!
//! Split out of `expand.rs` at its size ceiling. The expander models three
//! ways to give a variable a value: `NAME=value`, a `for NAME in …` list,
//! and the environment. Bash and zsh have many more: `read NAME`,
//! `printf -v NAME`, `print -v NAME`, `vared NAME`, `wait -p NAME`,
//! `declare -n R=NAME`, `${NAME:=…}`, `foreach NAME (…)`, a trap or an
//! `eval` string, and more. Rather than list them, this finds every name
//! the line spells in any OTHER way: a bare `NAME` token anywhere, or a
//! `${NAME` that is not plain `${NAME}`. Such a name is unknown wherever it
//! is read (ADR-0969). The cost is a refusal when a variable's name also
//! appears as an ordinary word.

use std::collections::HashSet;

/// The names *masked* (the line with its subshell spans masked) spells
/// other than as `$NAME`, `${NAME}`, a word-initial `NAME=`, or the
/// variable of `for NAME in`, together with every `${NAME…}` in *raw* that
/// is not plain; and whether the line may write ANY variable (`eval`,
/// `source`, `trap`, a nameref declaration).
pub(super) fn unmodelled_writes(masked: &str, raw: &str) -> (HashSet<String>, bool) {
    let mut names = HashSet::new();
    let c: Vec<char> = masked.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if !(c[i].is_ascii_alphabetic() || c[i] == '_') || (i > 0 && is_word_char(c[i - 1])) {
            i += 1;
            continue;
        }
        let end = i + c[i..].iter().take_while(|&&x| is_word_char(x)).count();
        let name: String = c[i..end].iter().collect();
        if !modelled(&c, i, end) {
            names.insert(name);
        }
        i = end;
    }
    names.extend(complex_references(raw));
    let words: Vec<&str> = masked
        .split(|ch: char| !is_word_char(ch) && ch != '-' && ch != '.')
        .collect();
    let any = words
        .iter()
        .any(|w| matches!(*w, "eval" | "source" | "trap"))
        || words.windows(2).any(|p| {
            matches!(p[0], "declare" | "typeset" | "local")
                && p[1].starts_with('-')
                && p[1].contains('n')
        })
        || masked.split(['\n', ';', '&', '|', '(']).any(|cmd| {
            let mut w = cmd.split_whitespace();
            let first = w.next();
            first == Some(".")
                || (matches!(first, Some("command" | "builtin" | "exec")) && w.next() == Some("."))
        });
    (names, any)
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether the name at `c[start..end]` is one of the spellings the
/// expander models.
fn modelled(c: &[char], start: usize, end: usize) -> bool {
    let before = start.checked_sub(1).map(|k| c[k]);
    // `$NAME`, and a plain `${NAME}`.
    if before == Some('$') {
        return true;
    }
    if before == Some('{') && start >= 2 && c[start - 2] == '$' && c.get(end) == Some(&'}') {
        return true;
    }
    let word_start =
        before.is_none_or(|b| b.is_whitespace() || matches!(b, ';' | '&' | '|' | '(' | '"' | '\''));
    // A word-initial `NAME=` (not `NAME==`, and not `NAME+=`).
    if word_start && c.get(end) == Some(&'=') && c.get(end + 1) != Some(&'=') {
        return true;
    }
    // `for NAME in`, read off the characters around it.
    let mut k = start;
    while k > 0 && c[k - 1].is_whitespace() {
        k -= 1;
    }
    let preceded_by_for = k >= 3
        && c[k - 3..k] == ['f', 'o', 'r']
        && (k == 3 || !is_word_char(c[k - 4]))
        && k < start;
    let mut j = end;
    while j < c.len() && c[j].is_whitespace() {
        j += 1;
    }
    let followed_by_in = j > end
        && c.get(j..j + 2) == Some(&['i', 'n'][..])
        && c.get(j + 2).is_none_or(|&x| !is_word_char(x));
    preceded_by_for && followed_by_in
}

/// Each NAME in a `${NAME…}` or `${!NAME…}` that is not plain `${NAME}`.
fn complex_references(raw: &str) -> Vec<String> {
    let c: Vec<char> = raw.chars().collect();
    let mut out = Vec::new();
    for i in 0..c.len() {
        if c[i] != '$' || c.get(i + 1) != Some(&'\u{7b}') {
            continue;
        }
        let mut k = i + 2;
        let bang = c.get(k) == Some(&'!');
        if bang {
            k += 1;
        }
        let end = k + c[k..].iter().take_while(|&&x| is_word_char(x)).count();
        if end > k && (bang || c.get(end) != Some(&'\u{7d}')) {
            out.push(c[k..end].iter().collect());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(line: &str) -> Vec<String> {
        let mut v: Vec<String> = unmodelled_writes(line, line).0.into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn only_the_modelled_spellings_leave_a_name_known() {
        assert!(names("D=/tmp; rm $D/x ${D}").iter().all(|n| n != "D"));
        assert!(names("for f in a b; do echo $f; done")
            .iter()
            .all(|n| n != "f"));
        for line in [
            "read D",
            "printf -v D x",
            "declare -n R=D",
            "vared D",
            "foreach D (a) echo; end",
            "x ${D:=a}",
            "x ${!D}",
        ] {
            assert!(names(line).contains(&"D".to_string()), "{line}");
        }
    }

    #[test]
    fn eval_source_trap_and_namerefs_may_write_anything() {
        for line in [
            "command eval x",
            "trap x DEBUG",
            ". ./f",
            "builtin . f",
            "local -n R",
        ] {
            assert!(unmodelled_writes(line, line).1, "{line}");
        }
        assert!(!unmodelled_writes("find . -name x", "find . -name x").1);
    }
}
