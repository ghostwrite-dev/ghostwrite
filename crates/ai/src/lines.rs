//! The lines an item is about. Files are shown to the model with their lines
//! numbered (`41│let x = 1;`), and it quotes lines back the same way in
//! `<at>`: the number says where to look, the text says what's there.
//!
//! [`find`] finds quoted lines in a file, forgiving what a copy tends to get
//! wrong: spacing and indentation, then (only near where a number points)
//! small slips in the text itself. [`change`] is what a step does to the
//! lines it found: lines it repeats are kept as the file has them, and its
//! new lines take the file's indentation if the quote got that wrong.

use std::ops::Range;

/// Between a line's number and its text, as the model is shown them.
pub const SEP: char = '│';
/// Separators a quoted line's number may end with: ours, and Claude Code's `Read`.
const SEPS: [char; 2] = [SEP, '→'];
/// How far from where a number points a close (not exact) copy is looked for.
const CLOSE_WITHIN: usize = 8;
/// How alike a close copy's lines must be: each one, and on average.
const CLOSE_EACH: f64 = 0.6;
const CLOSE_MEAN: f64 = 0.85;

/// Lines of a file, numbered from `first` (zero-based) as the model is shown
/// them; the lines `mark` (if any) between `<cursor>` and `</cursor>` lines.
pub fn numbered(text: &str, first: usize, mark: Option<Range<usize>>) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let n = first + i;
        if let Some(m) = mark.as_ref().filter(|m| m.start == n) {
            out.push_str(if m.is_empty() { "<cursor>\n</cursor>\n" } else { "<cursor>\n" });
        }
        out.push_str(&format!("{}{SEP}{line}\n", n + 1));
        if mark.as_ref().is_some_and(|m| !m.is_empty() && m.end == n + 1) {
            out.push_str("</cursor>\n");
        }
    }
    if mark.as_ref().is_some_and(|m| m.start >= first + lines.len()) {
        out.push_str("<cursor>\n</cursor>\n");
    }
    out
}

/// The whole lines of `text` around the lines `around`, numbered, in about
/// `bytes` of text; with `mark`, as [`numbered`] marks them.
pub fn window(text: &str, around: Range<usize>, bytes: usize, mark: bool) -> String {
    let starts = line_starts(text);
    let byte = |line: usize| starts.get(line).copied().unwrap_or(text.len());
    let half = bytes / 2;
    let from = byte(around.start).saturating_sub(half);
    let to = (byte(around.end) + half).min(text.len());
    // Whole lines: back to the start of the first, on to the end of the last.
    let first = starts.partition_point(|&s| s <= from).saturating_sub(1);
    let last = starts.partition_point(|&s| s < to);
    numbered(&text[byte(first)..byte(last)], first, mark.then_some(around))
}

/// The bytes `lines` take up in `text` (past its end: its end).
pub fn bytes_of(text: &str, lines: Range<usize>) -> Range<usize> {
    let at = |line: usize| match line.checked_sub(1) {
        None => 0,
        Some(before) => text.match_indices('\n').nth(before).map_or(text.len(), |(newline, _)| newline + 1),
    };
    let start = at(lines.start);
    start..at(lines.end).max(start)
}

/// The byte where each line of `text` starts.
pub fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1).filter(|&i| i < text.len())).collect()
}

/// Quoted lines without their numbers, and the (zero-based) line the first
/// one gave. They're taken as numbered only if every line starts with a
/// number and a separator, counting up by one, so code that happens to
/// start with digits stays as it is.
pub fn unnumbered(lines: &[String]) -> (Vec<String>, Option<usize>) {
    let split = |line: &String| {
        let rest = line.trim_start_matches(' ');
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        let sep = rest[digits..].chars().next().filter(|c| SEPS.contains(c))?;
        Some((rest[..digits].parse::<usize>().ok()?, rest[digits + sep.len_utf8()..].to_string()))
    };
    let Some(split): Option<Vec<(usize, String)>> = lines.iter().map(split).collect() else { return (lines.to_vec(), None) };
    let counts_up = split.windows(2).all(|w| Some(w[1].0) == w[0].0.checked_add(1));
    match split.first() {
        Some(&(n, _)) if counts_up && n > 0 => (split.into_iter().map(|(_, l)| l).collect(), Some(n - 1)),
        _ => (lines.to_vec(), None),
    }
}

/// How closely quoted lines matched the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Match {
    /// The same, but for trailing spaces.
    Exact,
    /// The same words: spacing or indentation differ.
    Spacing,
    /// Nearly the same, where the line number pointed.
    Close,
}

/// Where quoted lines are in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub lines: Range<usize>,
    pub how: Match,
}

/// Where `quoted` is in `text`, as whole lines: the copy nearest the lines
/// `near`, the closest kind of match first. `pointed`: `near` is where the
/// quote's line number points, so a copy right there is taken before an
/// exact one elsewhere, and a close copy near it will do. Nothing quoted is
/// `near` itself.
pub fn find(text: &str, quoted: &[String], near: Range<usize>, pointed: bool) -> Option<Found> {
    if quoted.is_empty() {
        return Some(Found { lines: near, how: Match::Exact });
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < quoted.len() {
        return None;
    }
    let starts = 0..=lines.len() - quoted.len();
    let at = |i: usize, how: Match| quoted.iter().enumerate().all(|(j, q)| same(lines[i + j], q, how));
    let found = |i: usize, how| Found { lines: i..i + quoted.len(), how };
    if pointed && starts.contains(&near.start) {
        for how in [Match::Exact, Match::Spacing] {
            if at(near.start, how) {
                return Some(found(near.start, how));
            }
        }
    }
    for how in [Match::Exact, Match::Spacing] {
        if let Some(i) = starts.clone().filter(|&i| at(i, how)).min_by_key(|&i| i.abs_diff(near.start)) {
            return Some(found(i, how));
        }
    }
    if !pointed {
        return None;
    }
    // A close copy: the most alike within reach of the number, unless another
    // about as alike is as near.
    let reach = near.start.saturating_sub(CLOSE_WITHIN)..=near.start.saturating_add(CLOSE_WITHIN).min(*starts.end());
    let away = |i: usize| i.abs_diff(near.start);
    let mut scored: Vec<(f64, usize)> = reach.filter_map(|i| Some((alike(&lines[i..i + quoted.len()], quoted)?, i))).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(away(a.1).cmp(&away(b.1))));
    let (best, i) = *scored.first()?;
    let rival = scored[1..].iter().any(|&(score, j)| best - score <= 0.05 && away(j) <= away(i));
    (!rival).then(|| found(i, Match::Close))
}

/// Where in `text` the lines most like `quoted` are, however unlike (to
/// point the model at them when nothing matched).
pub fn closest(text: &str, quoted: &[String]) -> Option<Range<usize>> {
    let lines: Vec<&str> = text.lines().collect();
    if quoted.is_empty() || lines.len() < quoted.len() {
        return None;
    }
    let score = |i: usize| quoted.iter().zip(&lines[i..]).map(|(q, l)| similarity(&words(q), &words(l))).sum::<f64>();
    let best = (0..=lines.len() - quoted.len()).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    (score(best) / quoted.len() as f64 > 0.4).then_some(best..best + quoted.len())
}

fn same(line: &str, quoted: &str, how: Match) -> bool {
    match how {
        Match::Exact => line.trim_end() == quoted.trim_end(),
        Match::Spacing | Match::Close => line.split_whitespace().eq(quoted.split_whitespace()),
    }
}

/// How alike `lines` are to `quoted`, on average, if every line is alike enough.
fn alike(lines: &[&str], quoted: &[String]) -> Option<f64> {
    let each: Vec<f64> = lines.iter().zip(quoted).map(|(l, q)| similarity(&words(l), &words(q))).collect();
    let mean = each.iter().sum::<f64>() / each.len() as f64;
    // Blank lines match anything blank; at least one line must have something to match.
    let substance = lines.iter().any(|l| l.trim().len() > 2);
    (substance && mean >= CLOSE_MEAN && each.iter().all(|&s| s >= CLOSE_EACH)).then_some(mean)
}

/// A line's words, single-spaced.
fn words(line: &str) -> Vec<char> {
    line.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect()
}

/// 1 for the same text, falling towards 0 the more edits apart they are.
fn similarity(a: &[char], b: &[char]) -> f64 {
    const LONGEST: usize = 400;
    if a == b {
        return 1.0;
    }
    if a.len().max(b.len()) > LONGEST {
        return 0.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(diagonal + usize::from(ca != cb));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    1.0 - row[b.len()] as f64 / a.len().max(b.len()) as f64
}

/// The visible part of a step: the lines that change, after dropping the
/// lines `old` and `new` share at either end.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// Lines struck out (empty: the new lines go in before this line).
    pub removed: Range<usize>,
    /// The lines that take their place.
    pub added: Vec<String>,
}

impl Change {
    /// The line the added lines show under.
    pub fn anchor_line(&self) -> usize {
        if self.removed.is_empty() { self.removed.start.saturating_sub(1) } else { self.removed.end - 1 }
    }
}

/// `old`, found in `text`, becoming `new`. Lines both start or end with
/// stay as they are in the file. A quote whose indentation was off has its
/// new lines indented as the file is.
pub fn change(text: &str, found: &Found, old: &[String], new: &[String]) -> Change {
    let at = found.lines.clone();
    if old.is_empty() {
        return Change { removed: at, added: new.to_vec() };
    }
    let same = |a: &String, b: &String| a.trim_end() == b.trim_end();
    let prefix = old.iter().zip(new).take_while(|(a, b)| same(a, b)).count();
    let rest = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(rest).take_while(|(a, b)| same(a, b)).count();
    let removed = at.start + prefix..at.end - suffix;
    let mut added = new[prefix..new.len() - suffix].to_vec();
    if found.how != Match::Exact {
        let file: Vec<&str> = text.lines().skip(at.start).take(at.len()).collect();
        if let Some((quoted, actual)) = old.iter().zip(&file).find_map(|(q, f)| (!q.trim().is_empty()).then(|| (indent(q), indent(f)))) {
            for line in added.iter_mut().filter(|l| quoted != actual && l.starts_with(quoted)) {
                *line = format!("{actual}{}", &line[quoted.len()..]);
            }
        }
    }
    Change { removed, added }
}

fn indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    fn at(text: &str, quoted: &[&str], near: usize, pointed: bool) -> Option<(Range<usize>, Match)> {
        find(text, &lines(quoted), near..near + 1, pointed).map(|f| (f.lines, f.how))
    }

    #[test]
    fn the_bytes_of_lines_are_by_the_lines_of_the_text() {
        let text = "→→\ntwo\nthree";
        assert_eq!(&text[bytes_of(text, 0..1)], "→→\n");
        assert_eq!(&text[bytes_of(text, 1..2)], "two\n");
        assert_eq!(&text[bytes_of(text, 2..3)], "three");
        assert_eq!(bytes_of(text, 9..12), text.len()..text.len(), "past the end: the end");
    }

    #[test]
    fn numbers_lines_and_marks_the_cursor() {
        assert_eq!(numbered("a\nb\nc\n", 9, Some(10..11)), "10│a\n<cursor>\n11│b\n</cursor>\n12│c\n");
        assert_eq!(numbered("a\n", 0, Some(1..1)), "1│a\n<cursor>\n</cursor>\n", "the end of the file");
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let shown = window(&text, 50..51, 40, false);
        assert!(shown.starts_with("4") && shown.contains("51│line 50\n") && shown.ends_with("\n"), "{shown}");
        assert!(!shown.contains("1│line 0\n"));
    }

    #[test]
    fn quoted_numbers_are_taken_off() {
        assert_eq!(unnumbered(&lines(&["41│    let x = 1;", "42│}"])), (lines(&["    let x = 1;", "}"]), Some(40)));
        assert_eq!(unnumbered(&lines(&["   7→fn a() {}"])), (lines(&["fn a() {}"]), Some(6)), "Claude Code's Read numbers lines too");
        assert_eq!(unnumbered(&lines(&["41│a", "43│b"])).1, None, "numbers that don't count up aren't numbers");
        assert_eq!(unnumbered(&lines(&["404 => not_found(),"])).1, None, "nor digits in code");
        assert_eq!(unnumbered(&lines(&["5│"])), (lines(&[""]), Some(4)), "a blank line");
    }

    #[test]
    fn finds_the_nearest_copy() {
        let text = "a\nx\nb\nx\nc\n";
        assert_eq!(at(text, &["x"], 3, false), Some((3..4, Match::Exact)));
        assert_eq!(at(text, &["x  "], 0, false), Some((1..2, Match::Exact)));
        assert_eq!(at(text, &["b", "x"], 0, false), Some((2..4, Match::Exact)));
        assert_eq!(at(text, &["nope"], 0, false), None);
        assert_eq!(at("def f():\n\treturn  1\n", &["    return 1"], 0, false), Some((1..2, Match::Spacing)), "spacing may differ");
        assert_eq!(find(text, &[], 4..5, false), Some(Found { lines: 4..5, how: Match::Exact }));
    }

    #[test]
    fn a_number_points_at_the_copy_meant() {
        let text = "fn a() {\n    x();\n    }\n}\n";
        assert_eq!(at(text, &["}"], 2, true), Some((2..3, Match::Spacing)), "where it points, before an exact copy elsewhere");
        assert_eq!(at(text, &["}"], 2, false), Some((3..4, Match::Exact)), "without a number, the exact copy");
    }

    #[test]
    fn a_close_copy_will_do_only_where_a_number_points() {
        let text = "fn main() {\n    let total = items.iter().sum();\n    println!(\"{total}\");\n}\n";
        let slip = ["    let total = item.iter().sum();"];
        assert_eq!(at(text, &slip, 1, true), Some((1..2, Match::Close)));
        assert_eq!(at(text, &slip, 1, false), None, "no number: no guessing");
        assert_eq!(at(text, &["    let other = something_else();"], 1, true), None, "too unlike");
        let far: String = std::iter::repeat_n("\n", 30).collect::<String>() + text;
        assert_eq!(at(&far, &slip, 1, true), None, "too far from where it points");
        assert_eq!(closest(&far, &lines(&slip)), Some(31..32), "but it can be pointed out");
    }

    #[test]
    fn a_change_keeps_the_lines_it_repeats() {
        let found = |lines: Range<usize>, how| Found { lines, how };
        let c = change("", &found(5..6, Match::Exact), &lines(&["fn a() {"]), &lines(&["fn a() {", "    x();"]));
        assert_eq!(c, Change { removed: 6..6, added: lines(&["    x();"]) });
        assert_eq!(c.anchor_line(), 5);
        let c = change("", &found(2..5, Match::Exact), &lines(&["a", "b", "c"]), &lines(&["a", "B", "c"]));
        assert_eq!(c, Change { removed: 3..4, added: lines(&["B"]) });
        let c = change("", &found(7..8, Match::Exact), &[], &lines(&["new"]));
        assert_eq!(c, Change { removed: 7..8, added: lines(&["new"]) });
    }

    #[test]
    fn new_lines_take_the_files_indentation() {
        let text = "def f():\n\tpass\n";
        let found = find(text, &lines(&["    pass"]), 0..1, false).unwrap();
        let c = change(text, &found, &lines(&["    pass"]), &lines(&["    x = 1", "    return x"]));
        assert_eq!(c, Change { removed: 1..2, added: lines(&["\tx = 1", "\treturn x"]) });
    }
}
