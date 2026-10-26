//! What a simple command actually runs: the program behind the wrappers, and
//! the command substitutions inside a line.
//!
//! Split out of `guard.rs` at its size ceiling. The seam is the question: every
//! rule asks this module what runs, and none of them is answered here.

use crate::install::shellword::SimpleCommand;

/// The program a simple command runs, and the arguments it gets.
///
/// `sudo`, `env`, `timeout`, `xargs` and friends are stripped because they do
/// not change what runs (`timeout 5 X` runs X), and a leading `FOO=1` is an
/// assignment rather than a program. So is a compound command's opener
/// ([`OPENERS`]): the scanner keeps `do` in `for …; do X; done` as the
/// first word, and reading it as the program skipped X for every rule.
pub(super) fn executed(command: &SimpleCommand) -> Option<(&str, &[String])> {
    let mut rest = command.words.as_slice();
    loop {
        while rest.first().is_some_and(|w| OPENERS.contains(&w.as_str())) {
            rest = &rest[1..];
        }
        // Environment assignments precede the program: `TF_LOG=trace X`.
        while rest
            .first()
            .is_some_and(|w| w.contains('=') && !w.starts_with('-'))
        {
            rest = &rest[1..];
        }
        // zsh reads `{cmd;}` as a group with no space after the brace.
        let first = rest.first()?;
        let first = first
            .strip_prefix(OPEN_BRACE)
            .filter(|w| !w.is_empty())
            .unwrap_or(first);
        let wrapper = program_name(first);
        let Some(options) = wrapper_options(wrapper) else {
            return Some((wrapper, &rest[1..]));
        };
        rest = skip_options(&rest[1..], &options);
        // A positional operand before the command: `timeout 5`.
        rest = rest.get(options.operands..)?;
    }
}

/// How one wrapper's options read, as `getopt` reads them.
struct WrapperOptions {
    /// Short options that take a value, attached (`-n1`) or as the next word.
    short_values: &'static str,
    /// Short options whose value may only be attached (`xargs -i{}`).
    short_attached: &'static str,
    /// Long options that take the next word as their value without `=`.
    long_values: &'static [&'static str],
    /// Operands the wrapper takes before the command it runs.
    operands: usize,
}

/// The option grammar of *wrapper*, or `None` when it is not a wrapper.
///
/// Per wrapper, because the same letter means different things: `env -i`
/// ignores the environment and takes nothing, while `env -u` names a variable
/// to unset. Getting that backwards eats the program name and the guard sees
/// something harmless where terraform was about to run.
fn wrapper_options(wrapper: &str) -> Option<WrapperOptions> {
    let (short_values, short_attached, long_values, operands): (_, _, &[&str], _) = match wrapper {
        "sudo" | "doas" => ("ugpChrtUTD", "", SUDO_LONG, 0),
        "env" => ("uSC", "", &["--unset", "--split-string", "--chdir"], 0),
        "nice" => ("n", "", &["--adjustment"], 0),
        "ionice" => ("ncp", "", &["--class", "--classdata", "--pid"], 0),
        "stdbuf" => ("ioe", "", &["--input", "--output", "--error"], 0),
        "time" => ("fo", "", &["--format", "--output"], 0),
        "exec" => ("a", "", &[], 0),
        "timeout" => ("sk", "", &["--signal", "--kill-after"], 1),
        "xargs" => ("adEILnPs", "eil", XARGS_LONG, 0),
        // Multi-call binaries: `busybox cp` runs cp.
        "nohup" | "command" | "builtin" | "setsid" | "busybox" | "toybox" => ("", "", &[], 0),
        // `coproc cmd` runs cmd; `coproc NAME { cmd; }` is split at the brace
        // (see [`split_at_braces`]). zsh's precommand modifiers run what
        // follows; `repeat N` takes the count first.
        "coproc" | "noglob" | "nocorrect" | "-" => ("", "", &[], 0),
        "repeat" => ("", "", &[], 1),
        _ => return None,
    };
    Some(WrapperOptions {
        short_values,
        short_attached,
        long_values,
        operands,
    })
}

const SUDO_LONG: &[&str] = &[
    "--user",
    "--group",
    "--prompt",
    "--close-from",
    "--host",
    "--role",
    "--type",
    "--other-user",
    "--command-timeout",
    "--chdir",
];

const XARGS_LONG: &[&str] = &[
    "--arg-file",
    "--delimiter",
    // NOT `--max-lines`: its value is optional, so only `--max-lines=N`
    // carries one, and `xargs --max-lines X` runs X.
    "--max-args",
    "--max-procs",
    "--max-chars",
    "--process-slot-var",
];

/// Whether long option *word* (no `=`) takes the next word as its value:
/// it is one of *long_values*, or a prefix of one, since `getopt_long`
/// accepts any unambiguous prefix (`--sig KILL` is `--signal KILL`).
pub(super) fn long_takes_a_value(word: &str, long_values: &[&str]) -> bool {
    word.len() > 2 && !word.contains('=') && long_values.iter().any(|l| l.starts_with(word))
}

/// *words* after the wrapper's leading options, `getopt`-style: a short
/// cluster is read letter by letter up to its first value-taking option, and
/// `--` ends the options. A lone `-` is skipped as main skipped it: to `env`
/// it means `-i`.
fn skip_options<'a>(mut words: &'a [String], options: &WrapperOptions) -> &'a [String] {
    while let Some(word) = words.first() {
        if word == "--" {
            return &words[1..];
        }
        let Some(flags) = word.strip_prefix('-') else {
            return words;
        };
        words = &words[1..];
        if flags.starts_with('-') {
            if long_takes_a_value(word, options.long_values) {
                words = words.get(1..).unwrap_or(&[]);
            }
            continue;
        }
        for (at, letter) in flags.char_indices() {
            if options.short_attached.contains(letter) {
                break;
            }
            if options.short_values.contains(letter) {
                if at + letter.len_utf8() == flags.len() {
                    words = words.get(1..).unwrap_or(&[]);
                }
                break;
            }
        }
    }
    words
}

/// How deeply `$(...)` substitutions nest in *line*, quoted or not, as
/// [`substitutions`] reads them: `echo $(a $(b))` is 2.
pub(super) fn substitution_depth(line: &str) -> usize {
    // One entry per open paren: whether it opened a substitution.
    let mut open: Vec<bool> = Vec::new();
    let (mut depth, mut deepest) = (0usize, 0usize);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                open.push(true);
                depth += 1;
                deepest = deepest.max(depth);
            }
            '(' => open.push(false),
            // The guard pops whatever the `)` closes.
            ')' if open.pop() == Some(true) => depth -= 1,
            _ => {}
        }
    }
    deepest
}

/// The text of every outermost `$(...)` and backtick substitution in
/// *line*, quoted or not; nested ones are part of their outer text.
///
/// Inside single quotes these are literal text to a shell; they are taken
/// anyway, because reading too much here can only refuse more.
pub(super) fn substitutions(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
            let start = i + 2;
            let mut depth = 1;
            let mut j = start;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let end = if depth == 0 { j - 1 } else { j };
            found.push(chars[start..end].iter().collect());
            // OUTERMOST ONLY, so the scan stays linear; a nested one is found
            // when its outer text is judged as a line.
            i = j;
        } else if chars[i] == '`' {
            let start = i + 1;
            let end = chars[start..]
                .iter()
                .position(|&c| c == '`')
                .map_or(chars.len(), |n| start + n);
            found.push(chars[start..end].iter().collect());
            i = end + 1;
        } else {
            i += 1;
        }
    }
    found
}

/// The name a shell would resolve, without its directory or its extension.
///
/// BOTH SEPARATORS, because this client ships to Windows and
/// `C:\tools\terraform.exe` spells its path the other way.
fn program_name(word: &str) -> &str {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    base.strip_suffix(".exe").unwrap_or(base)
}

/// The value of an option given either as `-c script` or as `-cscript`.
pub(super) fn value_of<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if arg == option {
            args.get(i + 1).map(String::as_str)
        } else {
            arg.strip_prefix(option).filter(|v| !v.is_empty())
        }
    })
}

/// What a shell's `-c` may run: every word after the first option cluster
/// that carries `c` (`-c`, `-lc`, `-ec`), plus a value attached to `-c`.
///
/// EVERY word, not the next one: the script is the first OPERAND, so
/// `bash -c -e 'X'` runs X, and the guard does not model which word that
/// is. A later word is `$0`, `$1`…, so reading it too only over-refuses.
pub(super) fn shell_scripts(args: &[String]) -> Vec<&str> {
    let cluster = |a: &String| {
        a.strip_prefix('-')
            .is_some_and(|f| !f.starts_with('-') && f.contains('c'))
    };
    let Some(at) = args.iter().position(cluster) else {
        return Vec::new();
    };
    let attached = value_of(&args[at..at + 1], "-c");
    attached
        .into_iter()
        .chain(args[at + 1..].iter().map(String::as_str))
        .collect()
}

const OPEN_BRACE: char = '\u{7b}';
const OPEN_WORD: &str = "\u{7b}";
const CLOSE_WORD: &str = "\u{7d}";

/// *commands* with each one split at a standalone open-brace word after
/// its first: what follows the brace is a command of its own, and a
/// trailing close brace is dropped. `coproc NAME`, `function f`, and zsh's
/// `if cond` / `while cond` keep the words before it. Splitting can only
/// add commands to judge.
pub(super) fn split_at_braces(commands: Vec<SimpleCommand>) -> Vec<SimpleCommand> {
    let paired = commands.into_iter().map(|c| (c, Vec::new())).collect();
    split_at_braces_with_separators(paired)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

/// [`split_at_braces`] for commands paired with the separators before
/// them. A command split off after the brace gets [`SPLIT_OFF`] as its
/// only separator: it runs on from the command before it.
pub(super) fn split_at_braces_with_separators(
    commands: Vec<(SimpleCommand, Vec<String>)>,
) -> Vec<(SimpleCommand, Vec<String>)> {
    let mut out = Vec::with_capacity(commands.len());
    for (command, mut run) in commands {
        let mut writes_to = command.writes_to;
        let mut words: Vec<String> = Vec::new();
        for (n, word) in command.words.into_iter().enumerate() {
            if n > 0 && word == OPEN_WORD {
                let before = SimpleCommand {
                    words: std::mem::take(&mut words),
                    writes_to: std::mem::take(&mut writes_to),
                };
                out.push((before, std::mem::replace(&mut run, vec![SPLIT_OFF.into()])));
                continue;
            }
            words.push(word);
        }
        while words.len() > 1 && words.last().is_some_and(|w| w == CLOSE_WORD) {
            words.pop();
        }
        out.push((SimpleCommand { words, writes_to }, run));
    }
    out
}

/// The separator a command split off at a brace gets. The scanner never
/// gives a command this one, so it cannot be mistaken for a real separator.
pub(super) const SPLIT_OFF: &str = OPEN_WORD;

/// Reserved words that open a compound command before its first simple
/// command. `time` is a wrapper, read with its options above.
pub(super) const OPENERS: &[&str] = &[
    "do", "then", "else", "elif", "if", "while", "until", "!", "\u{7b}", "(",
];

pub(super) const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];
