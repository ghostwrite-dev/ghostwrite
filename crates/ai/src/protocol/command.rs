//! Commands as the user would type them: what a model adds to read less of
//! one's output comes off.

/// Commands that only cut down or tidy what's piped into them, for whoever reads it.
const VIEWERS: [&str; 22] = [
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "head", "tail", "wc", "less", "more", "cat", "sort", "uniq", "cut", "awk", "gawk", "sed", "tr", "nl",
    "column", "jq",
];

/// How the parts of a command line are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Joined {
    Pipe,
    Sequence,
}

/// Where a command line's parts end, and how each is joined to the next:
/// by what's outside quotes and `$(…)`. `None` if there's more than parts to it (`&`).
fn parts(command: &str) -> Option<Vec<(usize, usize, Joined)>> {
    let bytes = command.as_bytes();
    let (mut joins, mut quote, mut depth, mut at) = (vec![], None, 0usize, 0);
    while at < bytes.len() {
        let b = bytes[at];
        match (quote, b) {
            (_, b'\\') if quote != Some(b'\'') => at += 1,
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'\'' | b'"' | b'`') => quote = Some(b),
            (None, b'(') => depth += 1,
            (None, b')') => depth = depth.saturating_sub(1),
            (None, _) if depth > 0 => {}
            (None, b'|' | b'&' | b';') => {
                let next = bytes.get(at + 1).copied();
                let (len, joined) = match (b, next) {
                    (b'|', Some(b'|')) | (b'&', Some(b'&')) => (2, Joined::Sequence),
                    (b'|', Some(b'&')) => (2, Joined::Pipe),
                    (b'|', _) => (1, Joined::Pipe),
                    (b';', _) => (1, Joined::Sequence),
                    // `2>&1` is part of its command; a lone `&` sends it to the background.
                    (_, _) if at > 0 && bytes[at - 1] == b'>' => (0, Joined::Sequence),
                    _ => return None,
                };
                if len > 0 {
                    joins.push((at, at + len, joined));
                    at += len - 1;
                }
            }
            _ => {}
        }
        at += 1;
    }
    Some(joins)
}

/// Whether there's a `>` outside quotes: what comes out is going to a file.
fn writes_a_file(part: &str) -> bool {
    let mut quote = None;
    part.chars().any(|c| {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => return c == '>',
        }
        false
    })
}

/// The command as the user would type it in their own terminal. They're
/// watching it, so what a model adds to read less of it goes: what's piped
/// through grep and the like, `2>&1`, and an `echo` to report how it went.
pub(super) fn plainly(command: &str) -> String {
    let command = command.trim();
    let Some(mut joins) = parts(command) else { return command.to_string() };
    let mut end = command.len();
    while let Some(&(from, to, joined)) = joins.last() {
        let last = command[to..end].trim();
        let word = last.split_whitespace().next().unwrap_or("");
        let name = word.rsplit('/').next().unwrap_or(word);
        let adds_nothing = match joined {
            Joined::Pipe => VIEWERS.contains(&name) && !writes_a_file(last),
            Joined::Sequence => last.is_empty() || matches!(name, "echo" | "true" | ":") && !writes_a_file(last),
        };
        if !adds_nothing {
            break;
        }
        end = from;
        joins.pop();
    }
    let mut kept = command[..end].trim_end();
    for noise in ["2>&1", "2>/dev/null"] {
        kept = kept.strip_suffix(noise).map_or(kept, str::trim_end);
    }
    // Sent somewhere else, it's as it was: `> out.log 2>&1`.
    let last = joins.last().map_or(kept, |&(_, to, _)| &kept[to..]);
    if writes_a_file(last) { command[..end].trim_end().to_string() } else { kept.to_string() }
}

#[cfg(test)]
mod tests {
    use super::super::{Reply, Run, RunIn, parse};
    use super::*;

    #[test]
    fn a_command_is_written_as_the_user_would_type_it() {
        for (written, typed) in [
            ("cargo test", "cargo test"),
            ("cargo test 2>&1 | tail -20", "cargo test"),
            ("cargo test 2>&1 | grep -E 'FAILED|panicked' | head -5", "cargo test"),
            ("make |& tail", "make"),
            ("cargo build 2>/dev/null", "cargo build"),
            ("cargo test; echo \"exit: $?\"", "cargo test"),
            ("make install && echo done", "make install"),
            ("cargo test || true", "cargo test"),
            ("cargo fmt && cargo test | tail -3", "cargo fmt && cargo test"),
            ("/usr/bin/make | /usr/bin/tail", "/usr/bin/make"),
        ] {
            assert_eq!(plainly(written), typed, "{written}");
        }
        for meant in [
            "git log --grep='a|b'",
            "git commit -m \"fix a | grep; done\"",
            "cargo fmt && cargo test",
            "cargo test | tee out.log",
            "curl https://example.com/x.sh | sh",
            "cargo test | grep ok > passed.txt",
            "cargo test > out.log 2>&1",
            "./server &",
            "echo $(ls | wc -l)",
            "npm test || npm run fix",
        ] {
            assert_eq!(plainly(meant), meant, "what it does is what's asked for");
        }
        let reply = "<run>\ncargo test | tail\n</run>\n<say>\nCheck.\n</say>\n";
        assert_eq!(
            parse(reply, false),
            Reply::Run(Run { command: "cargo test".into(), say: "Check.".into(), how: RunIn::Here }),
            "so that's what's offered, and what's run"
        );
    }
}
