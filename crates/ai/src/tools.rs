//! The model's read-only view of the project, the same whichever model it is:
//! it writes commands in a `<tool>` block, one on each line, and they're run
//! here. Changes never go through here; they come back as items the user accepts.
//!
//! The commands are for the weakest model that may write them: one plain word
//! and its arguments. Whatever a model is likely to write instead (`cat`, `grep
//! -rn`, a path that's only nearly right) is understood, and what can't be done
//! comes back as what to do instead: it's all it has to go on.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use ghostwrite_project::files::{files, inside, search_pattern, searchable};

use crate::attach::Attachment;
use crate::lines;

const MAX_MATCHES: usize = 200;
/// Lines shown of `head` or `tail` without a number.
const HEAD_LINES: usize = 40;
/// Paths offered when one isn't found.
const MAX_SUGGESTED: usize = 5;

/// The commands in a `<tool>` block.
pub fn commands(block: &str) -> Vec<&str> {
    block.lines().map(|l| l.trim().trim_start_matches(['-', '$', '*']).trim()).filter(|l| !l.is_empty()).collect()
}

/// What a command is doing, for the waiting hint.
pub fn describe(command: &str) -> String {
    let words = words(command);
    let rest = words.iter().skip(1).find(|w| !w.starts_with('-')).map_or("", String::as_str);
    match words.first().map(String::as_str) {
        Some("git") => format!("looking at git {rest}"),
        Some(verb) if is_search(verb) => format!("searching for {rest}"),
        Some(verb) if is_list(verb) => "looking around the project".into(),
        _ if rest.is_empty() => "looking around the project".into(),
        _ => format!("reading {rest}"),
    }
}

/// What a command came to: what it says, and the file it shows, if it's one to see.
pub struct Looked {
    pub text: String,
    pub file: Option<Attachment>,
}

/// [`run`], for a model that `sees` images and PDFs or one that doesn't: reading one
/// shows it, or says it can't be.
pub fn look(root: &Path, command: &str, room: usize, sees: bool) -> Looked {
    let words = words(command);
    let reads = words.first().is_some_and(|verb| ["read", "read_file", "cat", "open", "view", "less", "more", "show", "head", "tail"].contains(&verb.as_str()));
    let found = reads.then(|| path_arg(&words[1..])).flatten().and_then(|path| {
        // `a.png:12`, as for any file.
        let path = path.rsplit_once(':').filter(|(_, line)| line.parse::<usize>().is_ok()).map_or(path.clone(), |(file, _)| file.to_string());
        resolve(root, &path).ok()
    });
    let Some(found) = found.filter(|found| found.path.is_file()) else { return Looked { text: run(root, command, room), file: None } };
    let Some(kind) = crate::attach::kind_of(&found.path) else { return Looked { text: run(root, command, room), file: None } };
    if !sees {
        return Looked { text: format!("{}{} is {kind}, and this model can only read text.", found.note, found.rel), file: None };
    }
    match crate::attach::attach(&found.path) {
        Ok(file) => Looked { text: format!("{}{} is {kind}: it's shown to you here, as it is.", found.note, found.rel), file: Some(file) },
        Err(why) => Looked { text: format!("{}{} is {kind}, and it can't be shown: {why}.", found.note, found.rel), file: None },
    }
}

/// Run a command, for at most about `room` bytes of output. What goes wrong is
/// output too: it tells the model what to do instead.
pub fn run(root: &Path, command: &str, room: usize) -> String {
    let words = words(command);
    let Some(verb) = words.first() else { return USAGE.into() };
    let args = &words[1..];
    match verb.as_str() {
        "git" => git(root, args, room),
        "wc" => match path_arg(args).map(|p| resolve(root, &p)) {
            Some(Ok(found)) => match std::fs::read_to_string(&found.path) {
                Ok(text) => format!("{}: {} lines", found.rel, text.lines().count()),
                Err(e) => format!("couldn't read {}: {e}", found.rel),
            },
            Some(Err(why)) => why,
            None => USAGE.into(),
        },
        verb if is_search(verb) => search(root, args, room),
        verb if is_list(verb) => list(root, path_arg(args).as_deref().unwrap_or("."), room),
        "read" | "read_file" | "cat" | "open" | "view" | "less" | "more" | "show" => read(root, args, room, Part::Asked),
        "head" => read(root, args, room, Part::Head),
        "tail" => read(root, args, room, Part::Tail),
        other => format!("there's no command `{other}`. {USAGE}"),
    }
}

const USAGE: &str = "Write one of these on each line: read PATH [FIRST_LINE [LINES]], list [FOLDER], search PATTERN [FOLDER], git status|log|diff|show|blame.";

fn is_search(verb: &str) -> bool {
    matches!(verb, "search" | "search_files" | "grep" | "rg" | "ag" | "egrep" | "find_in_files")
}

fn is_list(verb: &str) -> bool {
    matches!(verb, "list" | "list_files" | "ls" | "tree" | "find" | "dir")
}

/// A command's words: quotes keep one together, and what would pipe or chain it
/// ends it. Punctuation a model carries over from a function call comes off.
fn words(command: &str) -> Vec<String> {
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut quoted = false;
    // A whole command in backticks, as if in prose.
    let command = command.trim().strip_prefix('`').and_then(|c| c.strip_suffix('`')).unwrap_or(command);
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if matches!(c, '"' | '\'' | '`') => {
                quote = Some(c);
                quoted = true;
            }
            None if c.is_whitespace() => {
                if !word.is_empty() || quoted {
                    words.push(std::mem::take(&mut word));
                }
                quoted = false;
            }
            None => word.push(c),
        }
    }
    if !word.is_empty() || quoted {
        words.push(word);
    }
    if let Some(end) = words.iter().position(|w| matches!(w.as_str(), "|" | "||" | "&&" | ";" | ">" | ">>" | "<" | "2>&1")) {
        words.truncate(end);
    }
    // `read_file(a.py)`.
    if words.len() == 1
        && let Some((verb, rest)) = words[0].clone().trim_end_matches(')').split_once('(')
    {
        words = vec![verb.to_string(), rest.to_string()];
    }
    // `read path=a.py`.
    words
        .into_iter()
        .map(|w| w.split_once('=').filter(|(k, _)| matches!(*k, "path" | "file" | "folder" | "pattern")).map_or(w.clone(), |(_, v)| v.to_string()))
        .collect()
}

/// The first argument that isn't a flag.
fn path_arg(args: &[String]) -> Option<String> {
    args.iter().find(|a| !a.starts_with('-')).cloned()
}

struct Found {
    path: PathBuf,
    rel: String,
    /// How a nearly right path was understood.
    note: String,
}

/// The file or folder a model means by `given`. A path that doesn't exist is
/// looked for by its end, so `util.py` finds `src/util.py`.
fn resolve(root: &Path, given: &str) -> Result<Found, String> {
    let root_text = root.display().to_string();
    let clean = given.strip_prefix(&root_text).unwrap_or(given);
    let clean = clean.trim_start_matches("./").trim_start_matches('/').trim_end_matches('/');
    let clean = if clean.is_empty() { "." } else { clean };
    if let Some(path) = inside(root, clean) {
        let real_root = ghostwrite_project::platform::real(root).unwrap_or_else(|_| root.to_path_buf());
        let rel = path.strip_prefix(&real_root).map_or_else(|_| clean.to_string(), |r| r.display().to_string());
        return Ok(Found { path, rel, note: String::new() });
    }
    let all = files(root, root);
    let suffix = format!("/{}", clean.trim_start_matches("../"));
    let name = clean.rsplit('/').next().unwrap_or(clean);
    let mut matches: Vec<&String> = all.iter().filter(|f| f.ends_with(&suffix) || f.rsplit('/').next() == Some(name)).collect();
    if matches.len() == 1 {
        let rel = matches[0].clone();
        return Ok(Found { path: root.join(&rel), note: format!("({given} is {rel})\n"), rel });
    }
    if matches.is_empty() {
        let stem = name.split('.').next().unwrap_or(name).to_lowercase();
        // Either way round: `library` for `lib.rs`, `lib` for `library.rs`.
        matches = all
            .iter()
            .filter(|f| {
                let file = f.rsplit('/').next().unwrap_or(f).to_lowercase();
                let file_stem = file.split('.').next().unwrap_or(&file).to_string();
                !stem.is_empty() && !file_stem.is_empty() && (file_stem.contains(&stem) || stem.contains(&file_stem))
            })
            .collect();
    }
    if matches.is_empty() {
        return Err(format!("there's no {given} in the project. `list` shows what there is."));
    }
    let suggested: Vec<&str> = matches.iter().take(MAX_SUGGESTED).map(|m| m.as_str()).collect();
    Err(format!("there's no {given} in the project. Did you mean one of these?\n{}", suggested.join("\n")))
}

enum Part {
    Asked,
    Head,
    Tail,
}

fn read(root: &Path, args: &[String], room: usize, part: Part) -> String {
    let mut numbers = vec![];
    let mut path = None;
    let mut flags = args.iter().peekable();
    while let Some(arg) = flags.next() {
        // `-n 20`, `-20`, `--lines=20`: only the number matters.
        let digits = arg.trim_start_matches('-').trim_start_matches("lines=");
        if arg.starts_with('-') && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            numbers.extend(digits.parse::<usize>());
        } else if arg.starts_with('-') {
            if flags.peek().is_some_and(|n| n.parse::<usize>().is_ok()) {
                numbers.extend(flags.next().and_then(|n| n.parse::<usize>().ok()));
            }
        } else if let (Some(_), Ok(n)) = (&path, arg.parse::<usize>()) {
            numbers.push(n);
        } else if path.is_none() {
            path = Some(arg.clone());
        }
    }
    let Some(mut given) = path else { return format!("read needs a path. {USAGE}") };
    // `a.py:12`: from that line.
    if let Some((file, line)) = given.clone().rsplit_once(':')
        && let Ok(line) = line.parse::<usize>()
    {
        given = file.to_string();
        numbers.insert(0, line);
    }
    let found = match resolve(root, &given) {
        Ok(found) => found,
        Err(why) => return why,
    };
    if found.path.is_dir() {
        return format!("{} is a folder:\n{}", found.rel, list(root, &found.rel, room));
    }
    let text = match std::fs::read_to_string(&found.path) {
        Ok(text) => text,
        Err(e) => return format!("couldn't read {}: {e}", found.rel),
    };
    let all: Vec<&str> = text.lines().collect();
    let (start, count) = match part {
        Part::Asked => (numbers.first().map_or(0, |n| n.saturating_sub(1)), numbers.get(1).copied()),
        Part::Head => (0, Some(numbers.first().copied().unwrap_or(HEAD_LINES))),
        Part::Tail => {
            let count = numbers.first().copied().unwrap_or(HEAD_LINES);
            (all.len().saturating_sub(count), Some(count))
        }
    };
    let start = start.min(all.len());
    let end = count.map_or(all.len(), |n| start.saturating_add(n).min(all.len()));
    // Whole lines, as many as there's room for.
    let mut shown = 0;
    let mut bytes = 0;
    while start + shown < end && (shown == 0 || bytes + all[start + shown].len() < room) {
        bytes += all[start + shown].len() + 8;
        shown += 1;
    }
    let body = lines::numbered(&all[start..start + shown].join("\n"), start, None);
    let last = start + shown;
    let hint = if last < end {
        format!("(lines {}-{last} of {}: `read {} {}` goes on)", start + 1, all.len(), found.rel, last + 1)
    } else if shown < all.len() {
        format!("(lines {}-{last} of {})", start + 1, all.len())
    } else {
        String::new()
    };
    format!("{}{body}{hint}", found.note)
}

fn list(root: &Path, given: &str, room: usize) -> String {
    let found = match resolve(root, given) {
        Ok(found) => found,
        Err(why) => return why,
    };
    if found.path.is_file() {
        return format!("{}{}", found.note, found.rel);
    }
    let real_root = ghostwrite_project::platform::real(root).unwrap_or_else(|_| root.to_path_buf());
    let all = files(&real_root, &found.path);
    if all.is_empty() {
        return "(no files)".into();
    }
    joined(&all, room, "more: `list` a folder")
}

/// Lines, as many as there's room for, and how many didn't fit.
fn joined(lines: &[String], room: usize, more: &str) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if !out.is_empty() && out.len() + line.len() >= room {
            out.push_str(&format!("(… {} {more})", lines.len() - i));
            return out;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn search(root: &Path, args: &[String], room: usize) -> String {
    let mut words: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
    // The last word may say where: a folder or file that exists.
    let real_root = ghostwrite_project::platform::real(root).unwrap_or_else(|_| root.to_path_buf());
    let mut under = real_root.clone();
    if words.len() > 1
        && let Some(path) = words.last().and_then(|w| inside(root, w))
    {
        under = path;
        words.pop();
    }
    if words.is_empty() {
        return format!("search needs something to look for. {USAGE}");
    }
    let pattern = match search_pattern(&words.join(" ")) {
        Ok(pattern) => pattern,
        Err(e) => return format!("that's not a pattern: {e}"),
    };
    let all = files(&real_root, &under);
    // Every core reads files: this is the one tool that may have a lot to do.
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let hits: Vec<String> = std::thread::scope(|scope| {
        let (pattern, real_root) = (&pattern, &real_root);
        let workers: Vec<_> = all
            .chunks(all.len().div_ceil(cores).max(1))
            .map(|chunk| {
                scope.spawn(move || {
                    let mut hits = vec![];
                    for rel in chunk {
                        let Some(text) = searchable(&real_root.join(rel)) else { continue };
                        for (i, line) in text.lines().enumerate().filter(|(_, l)| pattern.is_match(l)) {
                            hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                            if hits.len() >= MAX_MATCHES {
                                return hits;
                            }
                        }
                    }
                    hits
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).take(MAX_MATCHES).collect()
    });
    if hits.is_empty() {
        return format!("(no matches for {} in {} files: try a shorter pattern, or `list` the files)", words.join(" "), all.len());
    }
    joined(&hits, room, "more matches: search in a folder")
}

/// Git, reading only: flags that aren't known to only read are dropped
/// (`--output` writes a file), and what a repository's config can make git run
/// is off.
fn git(root: &Path, args: &[String], room: usize) -> String {
    const FLAGS: [&str; 5] = ["--", "--cached", "--staged", "--stat", "--name-only"];
    let what = args.first().map_or("status", String::as_str);
    let base: &[&str] = match what {
        "status" => &["status", "--short", "--branch"],
        "log" => &["log", "--oneline", "-n", "50"],
        "diff" => &["diff", "--no-ext-diff", "--no-textconv"],
        "show" => &["show", "--no-ext-diff", "--no-textconv"],
        "blame" => &["blame", "--no-textconv"],
        other => return format!("git {other} isn't for here: only status, log, diff, show and blame. To change something, reply with a <run> item."),
    };
    let rest = args.get(1..).unwrap_or_default();
    // A count after `-n` isn't a revision.
    let kept: Vec<&String> =
        rest.iter().filter(|a| (!a.starts_with('-') || FLAGS.contains(&a.as_str())) && !(what == "log" && a.parse::<usize>().is_ok())).collect();
    let output = ghostwrite_project::platform::command("git")
        .args(["-c", "core.fsmonitor=false", "--no-pager", "-C"])
        .arg(root)
        .args(base)
        .args(kept)
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => return format!("couldn't run git: {e}"),
    };
    if !output.status.success() {
        return String::from_utf8_lossy(&output.stderr).trim().to_string();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.is_empty() {
        return "(nothing)".into();
    }
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    joined(&lines, room, "more lines")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: usize = 100_000;

    fn project(name: &str) -> PathBuf {
        let root = ghostwrite_project::scratch_dir(name);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn one() {}\npub fn two() {}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/junk"), "x").unwrap();
        root
    }

    #[test]
    fn the_commands_read_inside_the_project_only() {
        let root = project("tools");
        assert_eq!(run(&root, "list", ROOM), ".gitignore\nsrc/lib.rs");
        assert_eq!(run(&root, "read src/lib.rs", ROOM), "1│pub fn one() {}\n2│pub fn two() {}\n");
        assert_eq!(run(&root, "search fn two", ROOM), "src/lib.rs:2: pub fn two() {}");
        assert!(run(&root, "read ../../etc/passwd", ROOM).contains("there's no"), "nothing outside the project");
        assert!(run(&root, "read /etc/passwd", ROOM).contains("there's no"));
    }

    #[test]
    fn what_a_model_is_likely_to_write_is_understood() {
        let root = project("tools-forgiving");
        let whole = "1│pub fn one() {}\n2│pub fn two() {}\n";
        for said in ["cat src/lib.rs", "read \"src/lib.rs\"", "read ./src/lib.rs", "read_file(src/lib.rs)", "read path=src/lib.rs", "`read src/lib.rs`"] {
            assert_eq!(run(&root, said, ROOM), whole, "{said}");
        }
        assert_eq!(run(&root, "read lib.rs", ROOM), format!("(lib.rs is src/lib.rs)\n{whole}"), "by its name");
        assert_eq!(run(&root, &format!("read {}/src/lib.rs", root.display()), ROOM), whole, "absolute");
        for said in ["head -n 1 src/lib.rs", "head -1 src/lib.rs", "read src/lib.rs 1 1"] {
            assert_eq!(run(&root, said, ROOM), "1│pub fn one() {}\n(lines 1-1 of 2)", "{said}");
        }
        for said in ["tail -n 1 src/lib.rs", "read src/lib.rs:2", "read src/lib.rs 2"] {
            assert_eq!(run(&root, said, ROOM), "2│pub fn two() {}\n(lines 2-2 of 2)", "{said}");
        }
        for said in ["grep -rn two .", "rg two src", "grep two src/lib.rs | head", "search \"fn two\""] {
            assert_eq!(run(&root, said, ROOM), "src/lib.rs:2: pub fn two() {}", "{said}");
        }
        for said in ["ls", "ls -la", "find .", "list_files"] {
            assert_eq!(run(&root, said, ROOM), ".gitignore\nsrc/lib.rs", "{said}");
        }
        assert_eq!(run(&root, "read src", ROOM), "src is a folder:\nsrc/lib.rs");
        assert_eq!(run(&root, "wc -l src/lib.rs", ROOM), "src/lib.rs: 2 lines");
    }

    #[test]
    fn what_fails_says_what_to_do_instead() {
        let root = project("tools-help");
        assert!(run(&root, "read library.rs", ROOM).contains("src/lib.rs"), "{}", run(&root, "read library.rs", ROOM));
        assert!(run(&root, "read nothing.txt", ROOM).contains("`list`"));
        assert!(run(&root, "summon src", ROOM).contains("read PATH"));
        assert!(run(&root, "search zzz", ROOM).contains("no matches for zzz"));
        assert!(run(&root, "search", ROOM).contains("needs something"));
    }

    #[test]
    fn a_long_file_is_cut_at_a_line_and_says_how_to_go_on() {
        let root = project("tools-long");
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("long.txt"), text).unwrap();
        let out = run(&root, "read long.txt", 200);
        assert!(out.starts_with("1│line 0\n") && out.len() < 400, "{out}");
        let hint = out.lines().last().unwrap();
        assert!(hint.contains("of 1000") && hint.contains("`read long.txt "), "{hint}");
        run(&root, "read long.txt 5000", ROOM);
    }

    #[test]
    fn git_only_reads() {
        let root = project("tools-git");
        std::process::Command::new("git").arg("init").arg("-q").arg(&root).status().unwrap();
        assert!(run(&root, "git status", ROOM).contains("?? src/"));
        let written = root.join("out");
        run(&root, &format!("git diff --output={}", written.display()), ROOM);
        assert!(!written.exists(), "the flag that writes is dropped");
        assert!(run(&root, "git push", ROOM).contains("<run>"));
    }

    /// How long the harness's own part takes, on this repository.
    #[test]
    #[ignore]
    fn it_is_fast() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        for command in [
            "list",
            "read crates/ai/src/backend.rs",
            "search fn read_chat_event",
            "search nothing_has_this_in_it_at_all",
            "read backend.rs",
            "git status",
            "git log",
        ] {
            let start = std::time::Instant::now();
            let out = run(&root, command, 8_000);
            println!("{:>7.2} ms  {command}  ({} bytes)", start.elapsed().as_secs_f64() * 1e3, out.len());
        }
        let all = files(&root, &root);
        let start = std::time::Instant::now();
        let grammar = crate::protocol::grammar(&all, true, false);
        println!("{:>7.2} ms  grammar ({} files, {} bytes)", start.elapsed().as_secs_f64() * 1e3, all.len(), grammar.len());
    }

    #[test]
    fn a_block_is_its_lines() {
        assert_eq!(commands("\nread a.py\n\n- search b\n$ list\n"), ["read a.py", "search b", "list"]);
    }

    #[test]
    fn it_says_what_a_command_is_doing() {
        assert_eq!(describe("read a.py"), "reading a.py");
        assert_eq!(describe("grep -rn foo"), "searching for foo");
        assert_eq!(describe("git log"), "looking at git log");
        assert_eq!(describe("ls"), "looking around the project");
    }
}
