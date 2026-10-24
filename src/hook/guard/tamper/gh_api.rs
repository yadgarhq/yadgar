//! Relaxation 3 of the exceptions-file check (yaadgaar #402, ADR-1005): the
//! endpoint of `gh api` is a URL path on GitHub, never a local file.
//!
//! `gh api repos/$r/git/refs/tags/$t` was refused: `$r` and `$t` read as
//! `*`, so `repos/*/…/*` may be `.claude/<file>`. To `gh` the word is an API
//! path, and no `gh api` flag writes a local file (`gh api --help`, gh 2.x:
//! `--input` and `-F key=@file` READ one, and stay checked).
//!
//! THE SHAPE, and nothing near it: the command's first word (after loop
//! `do`s) is exactly `gh` or `gh-personal`, not behind a wrapper; the second
//! is `api`; every option before and after is one `gh api` knows, so its
//! value is skipped rather than read as the endpoint; there is exactly one
//! operand; and that operand starts with a known API root (`repos/`,
//! `orgs/`, …, optionally after one `/`) and holds no `(`, backtick, `~`,
//! quote or whitespace. Only that one word leaves the check: every other
//! word, every redirect and the whole line's text are checked as before.

use crate::install::shellword::SimpleCommand;

/// The top-level REST paths, and `graphql`.
const API_ROOTS: &[&str] = &[
    "repos",
    "orgs",
    "users",
    "user",
    "gists",
    "search",
    "graphql",
    "notifications",
    "rate_limit",
    "enterprises",
    "teams",
    "installation",
    "app",
    "meta",
    "events",
    "issues",
    "licenses",
];

/// `gh api` options that take the next word as their value.
const VALUE_OPTIONS: &[&str] = &[
    "-X",
    "--method",
    "-H",
    "--header",
    "-f",
    "--raw-field",
    "-F",
    "--field",
    "-q",
    "--jq",
    "-t",
    "--template",
    "--input",
    "--hostname",
    "--cache",
    "-p",
    "--preview",
];

/// `gh api` options that take no value.
const FLAGS: &[&str] = &[
    "--paginate",
    "--slurp",
    "-i",
    "--include",
    "--silent",
    "--verbose",
    "--allow-escape-sequences",
];

/// The index in `command.words` of a `gh api` endpoint in the shape above.
pub(super) fn endpoint(command: &SimpleCommand) -> Option<usize> {
    let words = &command.words;
    let start = words.iter().take_while(|w| *w == "do").count();
    if !matches!(words.get(start)?.as_str(), "gh" | "gh-personal") || words.get(start + 1)? != "api"
    {
        return None;
    }
    let mut operand = None;
    let mut i = start + 2;
    while let Some(word) = words.get(i) {
        let name = word.split_once('=').map_or(word.as_str(), |(n, _)| n);
        if VALUE_OPTIONS.contains(&word.as_str()) {
            i += 2;
            continue;
        }
        if FLAGS.contains(&word.as_str())
            || (name.starts_with("--") && VALUE_OPTIONS.contains(&name))
        {
            i += 1;
            continue;
        }
        if word.starts_with('-') || operand.replace(i).is_some() {
            return None; // An option it does not know, or a second operand.
        }
        i += 1;
    }
    operand.filter(|&i| is_api_path(&words[i]))
}

/// Whether *word* is an API path: a known root, and a conservative charset.
fn is_api_path(word: &str) -> bool {
    let path = word.strip_prefix('/').unwrap_or(word);
    let root = path.split(['/', '?']).next().unwrap_or("");
    API_ROOTS.contains(&root)
        && !word
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "(`~'\"\\".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::shellword::shell_commands;

    fn of(line: &str) -> Option<usize> {
        endpoint(&shell_commands(line)[0])
    }

    #[test]
    fn only_the_one_operand_of_a_plain_gh_api_is_the_endpoint() {
        assert_eq!(of("gh api repos/$r/tags"), Some(2));
        assert_eq!(of("gh api -X GET -H a:b repos/$r --jq .x"), Some(6));
        assert_eq!(of("do gh api /repos/$r"), Some(3));
        assert_eq!(of("gh api --method=GET repos/$r"), Some(3));
        for line in [
            "gh pr view $r",
            "command gh api repos/$r",
            "gh api",
            "gh api repos/$a repos/$b",
            "gh api --what repos/$r",
            "gh api $D/$f",
            "gh api .claude/$f",
            "gh api repos/`x`",
            "gh api repos/~/x",
            "gh api -X",
        ] {
            assert_eq!(of(line), None, "{line}");
        }
    }
}
