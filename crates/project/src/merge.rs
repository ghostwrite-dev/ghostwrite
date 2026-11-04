//! Bringing another program's changes to a file into unsaved edits of it: a
//! three-way merge by lines, from the text both started from. Changes to
//! different lines both go in; where they touch the same lines (or lines
//! next to each other), there's no telling which should win, and nothing is
//! merged.

use std::ops::Range;

use similar::{DiffOp, TextDiff};

/// `ours` and `theirs`, both changed from `base`, as one; `None` if their
/// changes touch the same lines.
pub fn merge(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let (base_lines, our_lines, their_lines): (Vec<&str>, Vec<&str>, Vec<&str>) =
        (base.split_inclusive('\n').collect(), ours.split_inclusive('\n').collect(), theirs.split_inclusive('\n').collect());
    // Each side's changes: the base's lines replaced, and with what.
    let mut changes: Vec<(Range<usize>, &[&str], bool)> = vec![];
    for (other, theirs) in [(&our_lines, false), (&their_lines, true)] {
        for (old, new) in hunks(&base_lines, other) {
            changes.push((old, &other[new], theirs));
        }
    }
    changes.sort_by_key(|(old, _, theirs)| (old.start, old.end, *theirs));
    // The same change made on both sides goes in once.
    changes.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1);
    // Touching (or next to) each other: which goes first, or whether both should, isn't known.
    if changes.windows(2).any(|pair| pair[1].0.start <= pair[0].0.end) {
        return None;
    }
    let mut merged = String::with_capacity(ours.len().max(theirs.len()));
    let mut at = 0;
    for (old, new, _) in changes {
        base_lines[at..old.start].iter().for_each(|line| merged.push_str(line));
        new.iter().for_each(|line| merged.push_str(line));
        at = old.end;
    }
    base_lines[at..].iter().for_each(|line| merged.push_str(line));
    Some(merged)
}

/// What makes `from` into `to`, line by line: the bytes of `from` replaced,
/// and with what, first to last. Each change on its own, so what's between
/// them (the cursor, say) stays put.
pub fn edits(from: &str, to: &str) -> Vec<(Range<usize>, String)> {
    let (old, new): (Vec<&str>, Vec<&str>) = (from.split_inclusive('\n').collect(), to.split_inclusive('\n').collect());
    // Where each line starts, and the end.
    let starts = |lines: &[&str]| {
        let mut at = vec![0];
        for line in lines {
            at.push(at[at.len() - 1] + line.len());
        }
        at
    };
    let (old_at, new_at) = (starts(&old), starts(&new));
    hunks(&old, &new).into_iter().map(|(o, n)| (old_at[o.start]..old_at[o.end], to[new_at[n.start]..new_at[n.end]].to_string())).collect()
}

/// Where `other` differs from `base`: lines of the one for lines of the other.
fn hunks(base: &[&str], other: &[&str]) -> Vec<(Range<usize>, Range<usize>)> {
    let diff = TextDiff::from_slices(base, other);
    diff.ops()
        .iter()
        .filter_map(|op| match *op {
            DiffOp::Equal { .. } => None,
            DiffOp::Delete { old_index, old_len, new_index } => Some((old_index..old_index + old_len, new_index..new_index)),
            DiffOp::Insert { old_index, new_index, new_len } => Some((old_index..old_index, new_index..new_index + new_len)),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => Some((old_index..old_index + old_len, new_index..new_index + new_len)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::merge;

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\n";

    #[test]
    fn changes_to_different_lines_both_go_in() {
        let ours = "one\nTWO\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfour\nFIVE\nsix\n";
        assert_eq!(merge(BASE, ours, theirs).as_deref(), Some("one\nTWO\nthree\nfour\nFIVE\nsix\n"));
        assert_eq!(merge(BASE, BASE, theirs).as_deref(), Some(theirs), "nothing of ours: theirs");
        assert_eq!(merge(BASE, ours, BASE).as_deref(), Some(ours), "nothing of theirs: ours");
    }

    #[test]
    fn the_same_lines_changed_on_both_sides_dont_merge() {
        assert_eq!(merge(BASE, "one\nTWO\nthree\nfour\nfive\n", "one\n2\nthree\nfour\nfive\n"), None);
        assert_eq!(merge(BASE, "one\nTWO\nthree\nfour\nfive\n", "one\ntwo\nTHREE\nfour\nfive\n"), None, "next to each other");
        let same = "one\nTWO\nthree\nfour\nfive\n";
        assert_eq!(merge(BASE, same, same).as_deref(), Some(same), "unless it's the same change");
    }

    #[test]
    fn edits_make_one_text_the_other_change_by_change() {
        let to = "ONE\ntwo\nthree\nfour\nfive\nsix\n";
        let edits = super::edits(BASE, to);
        assert_eq!(edits, [(0..4, "ONE\n".to_string()), (BASE.len()..BASE.len(), "six\n".to_string())]);
        let mut text = BASE.to_string();
        for (range, with) in edits.into_iter().rev() {
            text.replace_range(range, &with);
        }
        assert_eq!(text, to);
    }

    #[test]
    fn lines_added_and_taken_away_merge() {
        let ours = "zero\none\ntwo\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfive\n";
        assert_eq!(merge(BASE, ours, theirs).as_deref(), Some("zero\none\ntwo\nthree\nfive\n"));
    }
}
