//! Selecting by the syntax tree, as helix does: out to the node around the selection
//! (`alt-o`) and back in (`alt-i`), to the next or previous node beside it (`alt-n`,
//! `alt-p`), all of them or all its children as selections (`alt-a`, `alt-I`), to the
//! ends of the node it's in (`alt-e`, `alt-b`); and the text objects a language's query
//! names (`maf` a function, `mia` an argument's inside) and going to the next or previous
//! one (`]f`, `[a`). A file's tree is parsed when it's asked for, and kept: once it's
//! edited, the tree is parsed again from the one before, which is quick.

use std::cell::RefCell;
use std::ops::Range as Bytes;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator as _, Tree};

use crate::buffer::Buffer;
use crate::language::Language;
use crate::selection::Range;
use crate::syntax::RopeText;

thread_local! {
    /// The last tree parsed: its language, the buffer and its version it's of, and the tree.
    static PARSED: RefCell<Option<(&'static Language, u64, u64, Tree)>> = const { RefCell::new(None) };
}

/// The buffer's syntax tree, if `language` has a grammar.
pub fn tree(language: &'static Language, text: &Buffer) -> Option<Tree> {
    let grammar = language.grammar?;
    PARSED.with(|parsed| {
        let mut parsed = parsed.borrow_mut();
        // The last one, if it's of this buffer: as it is, or edited as the buffer's been since.
        let old = match parsed.take() {
            Some((was_of, buffer, version, tree)) if std::ptr::eq(was_of, language) && buffer == text.id() => {
                if version == text.version() {
                    *parsed = Some((was_of, buffer, version, tree.clone()));
                    return Some(tree);
                }
                text.changes_since(version).map(|edits| {
                    let mut tree = tree;
                    edits.iter().for_each(|edit| tree.edit(edit));
                    tree
                })
            }
            _ => None,
        };
        let tree = grammar.parse_rope(text.rope(), old.as_ref(), &[])?;
        *parsed = Some((language, text.id(), text.version(), tree.clone()));
        Some(tree)
    })
}

/// A tree of the buffer as it was at `version` (worked out elsewhere, to color it),
/// kept to parse the next from if it's newer than the one kept: so the first `alt-o`
/// in a big file doesn't parse it all.
pub fn offer(language: &'static Language, text: &Buffer, version: u64, tree: &Tree) {
    PARSED.with(|parsed| {
        let mut parsed = parsed.borrow_mut();
        let older = match parsed.as_ref() {
            Some((was_of, buffer, kept, _)) => !std::ptr::eq(*was_of, language) || *buffer != text.id() || *kept < version,
            None => true,
        };
        if older && text.changes_since(version).is_some() {
            *parsed = Some((language, text.id(), version, tree.clone()));
        }
    });
}

/// The bytes a selection covers, as helix sees it: at least the char under the cursor.
fn covered(text: &Buffer, range: Range) -> Bytes<usize> {
    range.min_width_1(text).bytes()
}

fn forward(range: Range) -> bool {
    range.head >= range.anchor
}

/// `node`'s bytes as a selection, facing `forward` or back.
fn selecting(node: Node, forward: bool) -> Range {
    let (from, to) = (node.start_byte(), node.end_byte());
    if forward { Range::new(from, to) } else { Range::new(to, from) }
}

/// The smallest node that has all of `bytes` in it.
fn around<'t>(tree: &'t Tree, bytes: &Bytes<usize>) -> Node<'t> {
    let root = tree.root_node();
    root.descendant_for_byte_range(bytes.start, bytes.end).unwrap_or(root)
}

/// `alt-o`: the node around the selection, bigger than it.
pub fn expand(language: &'static Language, text: &Buffer, range: Range) -> Option<Range> {
    let tree = tree(language, text)?;
    let bytes = covered(text, range);
    let mut node = around(&tree, &bytes);
    while node.byte_range() == bytes {
        match node.parent() {
            Some(parent) => node = parent,
            None => break,
        }
    }
    Some(selecting(node, forward(range)))
}

/// `alt-i`: the first node inside the selection's.
pub fn shrink(language: &'static Language, text: &Buffer, range: Range) -> Option<Range> {
    let tree = tree(language, text)?;
    let node = around(&tree, &covered(text, range));
    let inner = node.named_child(0).or_else(|| node.child(0)).unwrap_or(node);
    Some(selecting(inner, forward(range)))
}

/// `alt-n`, `alt-p`: the node after (or before) the selection's, beside it or, if
/// it's the last there, beside the nearest node around it that isn't.
pub fn sibling(language: &'static Language, text: &Buffer, range: Range, next: bool) -> Option<Range> {
    let tree = tree(language, text)?;
    let mut node = around(&tree, &covered(text, range));
    loop {
        let beside = if next { node.next_named_sibling() } else { node.prev_named_sibling() };
        if let Some(beside) = beside {
            node = beside;
            break;
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => break,
        }
    }
    Some(selecting(node, next))
}

/// `alt-I`: each node inside the selection's, as selections; `alt-a`: each node beside
/// it, the selection's included. The selection itself if there are none.
pub fn spread(language: &'static Language, text: &Buffer, range: Range, siblings: bool) -> Option<Vec<Range>> {
    let tree = tree(language, text)?;
    let mut node = around(&tree, &covered(text, range));
    if siblings {
        loop {
            match node.parent() {
                Some(parent) if parent.child_count() > 1 => {
                    node = parent;
                    break;
                }
                Some(parent) => node = parent,
                None => return Some(vec![range]),
            }
        }
    }
    let mut walk = node.walk();
    let children: Vec<Range> = node.named_children(&mut walk).map(|child| selecting(child, forward(range))).collect();
    Some(if children.is_empty() { vec![range] } else { children })
}

/// `alt-e`, `alt-b`: to the end of the named node the selection's in (just past it), or
/// to its start, or, already there, its parent's. `extend` keeps the anchor.
pub fn node_end(language: &'static Language, text: &Buffer, range: Range, to_end: bool, extend: bool) -> Option<Range> {
    let tree = tree(language, text)?;
    let bytes = covered(text, range);
    let mut node = tree.root_node().named_descendant_for_byte_range(bytes.start, bytes.end)?;
    let head = if to_end {
        node.end_byte()
    } else {
        let start = node.start_byte();
        if start == range.cursor(text) {
            while let Some(parent) = node.parent() {
                node = parent;
                if node.start_byte() != start {
                    break;
                }
            }
            node.start_byte()
        } else {
            start
        }
    };
    let past = text.next(head);
    Some(if !extend {
        if forward(range) { Range::new(head, past) } else { Range::new(past, head) }
    } else if head >= range.anchor {
        Range::new(range.anchor, past)
    } else {
        Range::new(range.anchor, head)
    })
}

/// Every `name` (`function.around`, say) the language's query finds, as bytes: what one
/// match captures under it, together (a function and the attributes over it).
fn captured(language: &'static Language, tree: &Tree, text: &Buffer, names: &[String]) -> Vec<Bytes<usize>> {
    let Some(query) = query(language) else { return vec![] };
    let wanted: Vec<u32> = names.iter().filter_map(|name| query.capture_index_for_name(name)).collect();
    if wanted.is_empty() {
        return vec![];
    }
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), RopeText(text.rope()));
    let mut found = vec![];
    while let Some(m) = matches.next() {
        for index in &wanted {
            let nodes = m.captures().iter().filter(|c| c.index == *index).map(|c| c.node.byte_range());
            let together = nodes.reduce(|a, b| a.start.min(b.start)..a.end.max(b.end));
            found.extend(together);
        }
    }
    found
}

/// The language's text-object query.
fn query(language: &'static Language) -> Option<&'static Query> {
    language.grammar?.textobjects()
}

/// `mi`/`ma` with a syntax object (`f` function, `t` type, `a` argument, `c` comment,
/// `T` test, `e` entry): the smallest one around the cursor, inside or all of it.
pub fn object(language: &'static Language, text: &Buffer, range: Range, name: &str, around: bool) -> Option<Range> {
    let tree = tree(language, text)?;
    let at = range.cursor(text);
    let capture = format!("{name}.{}", if around { "around" } else { "inside" });
    let found = captured(language, &tree, text, &[capture]);
    let smallest = found.into_iter().filter(|bytes| bytes.start <= at && at < bytes.end.max(bytes.start + 1)).min_by_key(|bytes| bytes.len())?;
    Some(Range::new(smallest.start, smallest.end))
}

/// `]f`, `[f` and the like: the `count`th syntax object `name` after (or before) the
/// cursor, selected, facing the way it went.
pub fn goto_object(language: &'static Language, text: &Buffer, range: Range, name: &str, next: bool, count: usize) -> Option<Range> {
    let tree = tree(language, text)?;
    // The first of these the query has, as helix takes it.
    let names = ["movement", "around", "inside"].iter().map(|kind| format!("{name}.{kind}"));
    let first = query(language).and_then(|query| names.into_iter().find(|name| query.capture_index_for_name(name).is_some()))?;
    let found = captured(language, &tree, text, &[first]);
    let mut now = range;
    for _ in 0..count.max(1) {
        let at = now.cursor(text);
        let to = if next {
            found.iter().filter(|b| b.start > at).min_by_key(|b| (b.start, std::cmp::Reverse(b.end)))
        } else {
            found.iter().filter(|b| b.end < at).max_by_key(|b| (b.end, std::cmp::Reverse(b.start)))
        };
        let Some(to) = to else { break };
        let went = if next { Range::new(to.start, to.end) } else { Range::new(to.end, to.start) };
        if went == now {
            break;
        }
        now = went;
    }
    (now != range).then_some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> &'static Language {
        crate::language::testing::install();
        Language::for_path(std::path::Path::new("a.rs")).unwrap()
    }

    const CODE: &str = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn two() -> i32 {\n    add(1, 1)\n}\n";

    fn code() -> Buffer {
        Buffer::new(CODE)
    }

    fn at(text: &str, what: &str) -> Range {
        Range::point(text.find(what).unwrap())
    }

    fn shown(text: &str, range: Range) -> &str {
        &text[range.bytes()]
    }

    #[test]
    fn out_and_back_in() {
        let b = at(CODE, "b: i32");
        // The cursor already covers all of the name `b`: out to the parameter.
        let out = expand(rust(), &code(), b).unwrap();
        assert_eq!(shown(CODE, out), "b: i32", "the parameter");
        let out = expand(rust(), &code(), out).unwrap();
        assert_eq!(shown(CODE, out), "(a: i32, b: i32)", "all of them");
        let back = shrink(rust(), &code(), out).unwrap();
        assert_eq!(shown(CODE, back), "a: i32", "the first inside");
    }

    #[test]
    fn beside_and_all_of_them() {
        let a = expand(rust(), &code(), at(CODE, "a: i32")).unwrap();
        assert_eq!(shown(CODE, a), "a: i32");
        let b = sibling(rust(), &code(), a, true).unwrap();
        assert_eq!(shown(CODE, b), "b: i32", "the next one");
        assert_eq!(shown(CODE, sibling(rust(), &code(), b, false).unwrap()), "a: i32", "and back");
        let all: Vec<&str> = spread(rust(), &code(), a, true).unwrap().into_iter().map(|r| shown(CODE, r)).collect();
        assert_eq!(all, ["a: i32", "b: i32"], "each beside it");
        let params = expand(rust(), &code(), a).unwrap();
        let inside: Vec<&str> = spread(rust(), &code(), params, false).unwrap().into_iter().map(|r| shown(CODE, r)).collect();
        assert_eq!(inside, ["a: i32", "b: i32"], "each inside it");
    }

    #[test]
    fn to_the_ends_of_a_node() {
        let plus = at(CODE, "+ b");
        let end = node_end(rust(), &code(), plus, true, false).unwrap();
        assert_eq!(end.from(), CODE.find("+ b").unwrap() + 3, "just past `a + b`");
        let start = node_end(rust(), &code(), plus, false, false).unwrap();
        assert_eq!(start.from(), CODE.find("a + b").unwrap(), "its start");
    }

    #[test]
    fn functions_arguments_and_where_they_are() {
        let inside = at(CODE, "add(1, 1)");
        let function = object(rust(), &code(), inside, "function", true).unwrap();
        assert_eq!(shown(CODE, function), "fn two() -> i32 {\n    add(1, 1)\n}");
        let body = object(rust(), &code(), inside, "function", false).unwrap();
        assert!(shown(CODE, body).contains("add(1, 1)") && !shown(CODE, body).contains("fn two"), "{:?}", shown(CODE, body));
        let argument = object(rust(), &code(), at(CODE, "b: i32"), "parameter", false).unwrap();
        assert_eq!(shown(CODE, argument), "b: i32");

        let next = goto_object(rust(), &code(), Range::point(0), "function", true, 1).unwrap();
        assert_eq!(shown(CODE, next), "fn two() -> i32 {\n    add(1, 1)\n}", "]f");
        let back = goto_object(rust(), &code(), Range::point(CODE.len() - 1), "function", false, 2).unwrap();
        assert!(shown(CODE, back).starts_with("fn add"), "2[f");
        assert_eq!(goto_object(rust(), &code(), next, "function", true, 1), None, "there's no third");
    }

    #[test]
    fn the_text_object_query_loads() {
        assert!(query(rust()).is_some());
    }

    #[test]
    fn without_a_grammar_there_are_none() {
        static PLAIN: Language = Language { name: "plain", ..Language::DEFAULT };
        assert_eq!(expand(&PLAIN, &Buffer::new("text"), Range::point(0)), None);
    }
}
