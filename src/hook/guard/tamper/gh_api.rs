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
//! quote or whitespace; and no segment after the root is `..`, globs
//! (`*?[`), or holds an expansion that is not the whole segment
//! ([`is_api_segment`]). Only that one word leaves the check: every other
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

/// Whether *word* is an API path: a known root, a conservative charset, and
/// segments that are each a literal or one whole expansion.
fn is_api_path(word: &str) -> bool {
    let path = word.strip_prefix('/').unwrap_or(word);
    let mut segments = path.split('/');
    API_ROOTS.contains(&segments.next().unwrap_or(""))
        && !word
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "(`~'\"\\".contains(c))
        && segments.all(is_api_segment)
}

/// One segment after the root (security review of #87): never `..`; no
/// `*`, `?` or `[` outside an expansion, so nothing globs and there is no
/// query string; and a `$` only as the WHOLE segment, `$NAME` or `${…}`
/// holding no further `$`, `{` or `}`.
fn is_api_segment(segment: &str) -> bool {
    if segment == ".." {
        return false;
    }
    if let Some(name) = segment.strip_prefix('$') {
        if let Some(inner) = name.strip_prefix('{').and_then(|n| n.strip_suffix('}')) {
            return !inner.is_empty() && !inner.contains(['$', '{', '}']);
        }
        let mut chars = name.chars();
        return chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    }
    !segment.contains(['$', '*', '?', '['])
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

    #[test]
    fn an_endpoint_with_dot_dot_a_glob_or_a_partial_expansion_is_not_the_shape() {
        // Security review of #87: `..` climbs, `*?[` glob, and an expansion
        // inside a segment can carry `/` into its neighbours. Each defers to
        // main's check.
        for line in [
            "gh api repos/../.claude/x",
            "gh api repos/$r/../$t",
            "gh api repos/$r/..",
            "gh api /repos/a/../../b",
            "gh api repos/$r/*",
            "gh api repos/$r/x*",
            "gh api repos/$r/?",
            "gh api repos/$r/tags?per_page=1",
            "gh api repos/$r/[a-z]",
            "gh api repos/$r/x$t",
            "gh api repos/$r/${t}x",
            "gh api repos/a$r/tags",
            "gh api repos/$r$t",
            "gh api repos/$1/tags",
            "gh api repos/$r/${t/x/$y}",
        ] {
            assert_eq!(of(line), None, "{line}");
        }
        // Whole-segment expansions stay the shape.
        for line in [
            "gh api repos/$r/git/refs/tags/$t",
            "gh api repos/yadgarhq/${r%%:*}/git/ref/tags/${r#*:}",
            "gh api repos/{owner}/{repo}/git/refs/tags/$t",
            "gh api /repos/$r/tags",
            "gh-personal api repos/$r_1/tags",
        ] {
            assert!(of(line).is_some(), "{line}");
        }
    }
}
