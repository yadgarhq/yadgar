//! Redirects that write a file but do not scan as `>` (ADR-1020).
//!
//! `>|` and `>>|` (and zsh's `>!`, `>>!`) override noclobber, and `>&word`
//! with a *word* that is not a descriptor writes stdout and stderr into it.
//! The scanner reads the `|` or `&` as a separator, so the target lands in
//! program position, where the gh-shadowing checks skip it. The line those
//! checks read is respelled with a plain `>` first. `2>&1`, `>&2` and `>&-`
//! stay as they are: they write no file.
//!
//! Only the ADR-1020 checks read the respelled line, so no earlier refusal
//! changes (ADR-0645).

/// *line* with every clobbering or `>&word` redirect spelled `>` or `>>`,
/// or `None` when it holds none.
pub(super) fn plain(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        out.push(c);
        i += 1;
        if c == '\\' {
            // An escaped character is a literal, never an operator.
            if let Some(&next) = chars.get(i) {
                out.push(next);
                i += 1;
            }
            continue;
        }
        if c != '>' {
            continue;
        }
        if chars.get(i) == Some(&'>') {
            out.push('>');
            i += 1;
        }
        let respell = match chars.get(i) {
            Some('|' | '!') => true,
            Some('&') => chars
                .get(i + 1)
                .is_none_or(|n| !n.is_ascii_digit() && *n != '-'),
            _ => false,
        };
        if respell {
            i += 1;
            changed = true;
        }
    }
    changed.then_some(out)
}
