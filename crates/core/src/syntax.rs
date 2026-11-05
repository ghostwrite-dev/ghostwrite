//! Syntax with tree-sitter: a language's [`Grammar`], however it's run (a plugin's,
//! compiled to WebAssembly), its syntax tree, and the highlighted spans of its
//! text, in scopes the themes know.
//!
//! A file's colors ([`Colors`]) are kept as it's edited, as helix and zed keep
//! theirs: its tree is edited and parsed again from the old one, and only what
//! changed (and the lines edited) is colored again, the rest moved along. That's
//! done off the UI thread, on a snapshot of the text; what's shown meanwhile is
//! the last colors, moved past the edits since ([`spans_in`]).

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{Mutex, OnceLock, RwLock};

use ropey::Rope;
pub use tree_sitter::InputEdit;
use tree_sitter::{Node, Parser, Point, Query, QueryCursor, StreamingIterator as _, TextProvider, Tree};

use crate::language::Language;
/// The scopes spans are tagged with; themes color them by name, falling
/// back to the part before the last dot (`function.method` → `function`).
pub const SCOPES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "variable.special",
];

/// What other editors' queries call scopes (Helix, nvim-treesitter), as
/// ours, so a query from anywhere colors without being rewritten. A name
/// not here falls back to its shorter forms (`keyword.control.import` →
/// `keyword`).
const ALIASES: &[(&str, &str)] = &[
    ("character", "string.special"),
    ("constant.builtin.boolean", "boolean"),
    ("constant.character", "string.special"),
    ("constant.character.escape", "escape"),
    ("constant.numeric", "number"),
    ("markup.bold", "text.strong"),
    ("markup.heading", "text.title"),
    ("markup.heading.marker", "punctuation.special"),
    ("markup.italic", "text.emphasis"),
    ("markup.link", "text.reference"),
    ("markup.link.url", "text.uri"),
    ("markup.list", "punctuation.special"),
    ("markup.quote", "text.literal"),
    ("markup.raw", "text.literal"),
    ("markup.strong", "text.strong"),
    ("namespace", "module"),
    ("special", "punctuation.special"),
    ("string.special.url", "text.uri"),
    ("type.enum.variant", "constructor"),
    ("variable.member", "property"),
    ("variable.other.member", "property"),
];

/// Files bigger than this aren't colored by a grammar; plugin code, which colors
/// a file all over again on every change, stops at [`MAX_HIGHLIGHT_BY_CODE`].
const MAX_HIGHLIGHT: usize = 16 << 20;
const MAX_HIGHLIGHT_BY_CODE: usize = 1 << 20;

/// More edits than this at once, and it's quicker to color everything again.
const MAX_EDITS: usize = 256;

/// Languages inside languages inside languages: no deeper than this.
const MAX_DEPTH: usize = 4;

/// A highlighted stretch of text and its scope (an index into [`SCOPES`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub scope: usize,
}

/// The capture names other editors' queries use (ours, and theirs), and the scope each shows as.
fn names() -> &'static (Vec<&'static str>, Vec<Option<usize>>) {
    static NAMES: OnceLock<(Vec<&'static str>, Vec<Option<usize>>)> = OnceLock::new();
    NAMES.get_or_init(|| {
        let names: Vec<&str> = SCOPES.iter().copied().chain(ALIASES.iter().map(|(theirs, _)| *theirs)).collect();
        let scopes = names.iter().map(|n| scope(n)).collect();
        (names, scopes)
    })
}

/// The scope a query's capture shows as, as tree-sitter's highlighter picks it: the
/// name with the most parts that are all among the capture's (`keyword.control` →
/// `keyword`), and that name's scope.
fn capture_scope(capture: &str) -> Option<usize> {
    let parts: Vec<&str> = capture.split('.').collect();
    let (names, scopes) = names();
    let mut best: Option<(usize, usize)> = None;
    for (i, name) in names.iter().enumerate() {
        let len = name.split('.').count();
        if name.split('.').all(|part| parts.contains(&part)) && best.is_none_or(|(_, longest)| len > longest) {
            best = Some((i, len));
        }
    }
    scopes[best?.0]
}

/// What loads a grammar, and parses with it. A plugin's runtime has one for the
/// grammars it brings, compiled to WebAssembly.
pub trait Load: Send + Sync {
    /// The grammar itself.
    fn language(&self) -> Result<tree_sitter::Language, String>;
    /// A parser that can parse with it.
    fn parser(&self) -> Result<Parser, String>;
}

/// A grammar, with its queries: what colors a language (`highlights`), the other
/// languages inside it (`injections`: a code block's, say), and its text objects
/// (`textobjects`: what `maf` and `]f` select, as helix names them). It's loaded,
/// and its queries compiled, when it's first needed.
pub struct Grammar {
    pub name: String,
    load: Box<dyn Load>,
    highlights: String,
    injections: String,
    textobjects: String,
    loaded: OnceLock<Result<Loaded, String>>,
}

struct Loaded {
    language: tree_sitter::Language,
    highlights: Query,
    /// The scope each of its captures shows as, if any.
    scopes: Vec<Option<usize>>,
    injections: Option<Injections>,
    textobjects: Option<Query>,
    /// Parsers not in use, to use again: making one can take a while.
    parsers: Mutex<Vec<Parser>>,
}

/// The query of what other languages are inside one, and its captures that say which and where.
struct Injections {
    query: Query,
    content: Option<u32>,
    language: Option<u32>,
}

/// Grammars there to be injected only (a markup language's inline text, say), not a language's own.
static GRAMMARS: RwLock<Vec<&'static Grammar>> = RwLock::new(Vec::new());

impl Grammar {
    pub fn new(name: &str, load: Box<dyn Load>, highlights: &str, injections: &str, textobjects: &str) -> Self {
        Self {
            name: name.to_string(),
            load,
            highlights: highlights.to_string(),
            injections: injections.to_string(),
            textobjects: textobjects.to_string(),
            loaded: OnceLock::new(),
        }
    }

    /// Add a grammar that's injected by its name, and isn't a language's.
    pub fn register(grammar: &'static Grammar) {
        GRAMMARS.write().unwrap_or_else(|e| e.into_inner()).push(grammar);
    }

    pub fn unregister(grammar: &'static Grammar) {
        GRAMMARS.write().unwrap_or_else(|e| e.into_inner()).retain(|known| !std::ptr::eq(*known, grammar));
    }

    fn loaded(&self) -> Result<&Loaded, &str> {
        let loaded = self.loaded.get_or_init(|| {
            let language = self.load.language()?;
            let highlights = Query::new(&language, &self.highlights).map_err(|e| format!("its highlight query doesn't fit it: {e}"))?;
            let scopes = highlights.capture_names().iter().map(|name| capture_scope(name)).collect();
            let injections = (!self.injections.trim().is_empty())
                .then(|| Query::new(&language, &self.injections).map_err(|e| format!("its injection query doesn't fit it: {e}")))
                .transpose()?
                .map(|query| Injections {
                    content: query.capture_index_for_name("injection.content"),
                    language: query.capture_index_for_name("injection.language"),
                    query,
                });
            let textobjects = (!self.textobjects.is_empty())
                .then(|| Query::new(&language, &self.textobjects).map_err(|e| format!("its text-object query doesn't fit it: {e}")))
                .transpose()?;
            Ok(Loaded { language, highlights, scopes, injections, textobjects, parsers: Mutex::default() })
        });
        loaded.as_ref().map_err(|e| {
            static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
            let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
            if !said.contains(&self.name) {
                eprintln!("the {} grammar can't be used: {e}", self.name);
                said.push(self.name.clone());
            }
            e.as_str()
        })
    }

    /// Whether it loads, its queries fit it and it parses, or what's wrong.
    pub fn check(&self) -> Result<(), String> {
        self.loaded()?;
        self.parse("").map(drop).ok_or_else(|| "it doesn't parse".to_string())
    }

    /// `text`'s syntax tree.
    pub fn parse(&self, text: &str) -> Option<Tree> {
        self.parse_rope(&Rope::from_str(text), None, &[])
    }

    /// `text`'s syntax tree, read a piece of the rope at a time: from `old` (its tree
    /// before the edits since, each already given to it with [`Tree::edit`]), which is
    /// quicker, and only `ranges` of it, if any are given (a language inside another).
    pub fn parse_rope(&self, text: &Rope, old: Option<&Tree>, ranges: &[tree_sitter::Range]) -> Option<Tree> {
        let loaded = self.loaded().ok()?;
        let parser = loaded.parsers.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let mut parser = match parser {
            Some(parser) => parser,
            None => {
                let mut parser = self.load.parser().inspect_err(|e| eprintln!("the {} grammar can't parse: {e}", self.name)).ok()?;
                parser.set_language(&loaded.language).ok()?;
                parser
            }
        };
        let len = text.len_bytes();
        let mut read = |at: usize, _: Point| -> &[u8] {
            if at >= len {
                return &[];
            }
            let (chunk, start, _, _) = text.chunk_at_byte(at);
            &chunk.as_bytes()[at - start..]
        };
        let tree = parser.set_included_ranges(ranges).ok().and_then(|()| parser.parse_with_options(&mut read, old, None));
        if !ranges.is_empty() {
            parser.set_included_ranges(&[]).ok();
        }
        loaded.parsers.lock().unwrap_or_else(|e| e.into_inner()).push(parser);
        tree
    }

    /// Its text-object query, if it has one.
    pub fn textobjects(&self) -> Option<&Query> {
        self.loaded().ok()?.textobjects.as_ref()
    }

    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        let text = Rope::from_str(text);
        let Some(tree) = self.parse_rope(&text, None, &[]) else { return vec![] };
        self.spans(&tree, &text, 0..text.len_bytes())
    }

    /// The spans of `text` in `range` (and only there), its tree being `tree`: its own
    /// colors, and over them those of the languages inside it.
    pub fn spans(&self, tree: &Tree, text: &Rope, range: Range<usize>) -> Vec<Span> {
        let whole = tree_sitter::Range { start_byte: 0, end_byte: usize::MAX, start_point: Point::new(0, 0), end_point: Point::new(usize::MAX, usize::MAX) };
        layer(self, tree, text, &range, &[whole], 0)
    }
}

/// The scope a capture shows as: its name, or what it's an alias of, or
/// the same for a shorter form of it.
fn scope(capture: &str) -> Option<usize> {
    let mut name = capture;
    loop {
        let ours = ALIASES.iter().find(|(theirs, _)| *theirs == name).map_or(name, |(_, ours)| ours);
        if let Some(scope) = SCOPES.iter().position(|s| *s == ours) {
            return Some(scope);
        }
        name = &name[..name.rfind('.')?];
    }
}

/// The scope called `name` (or its alias, or a shorter form), as [`Span::scope`] numbers it.
pub fn scope_named(name: &str) -> Option<usize> {
    scope(name)
}

/// `top` laid over `base` (both in order, not overlapping): where a span of
/// `top` is, it wins; `base` shows around it.
pub fn overlay(base: &[Span], top: &[Span]) -> Vec<Span> {
    let mut out = Vec::with_capacity(base.len() + top.len());
    let mut tops = top.iter().peekable();
    // Where the last `top` span ended: `base` shows only after it.
    let mut covered = 0;
    for span in base {
        let mut from = span.range.start.max(covered);
        // The spans of `top` before or inside this one.
        while let Some(t) = tops.next_if(|t| t.range.start < span.range.end) {
            if t.range.start > from {
                out.push(Span { range: from..t.range.start, scope: span.scope });
            }
            (from, covered) = (from.max(t.range.end), t.range.end);
            out.push(t.clone());
        }
        if from < span.range.end {
            out.push(Span { range: from..span.range.end, scope: span.scope });
        }
    }
    out.extend(tops.cloned());
    out
}

/// `spans` of `old` moved onto `new`, which is `old` edited in one place:
/// those before the edit stay, those after it move with the text, and
/// what it replaced loses its colors.
pub fn follow_edit(spans: &[Span], old: &str, new: &str) -> Vec<Span> {
    let prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old.bytes().rev().zip(new.bytes().rev()).take(room).take_while(|(a, b)| a == b).count();
    let edit = InputEdit {
        start_byte: prefix,
        old_end_byte: old.len() - suffix,
        new_end_byte: new.len() - suffix,
        start_position: Point::default(),
        old_end_position: Point::default(),
        new_end_position: Point::default(),
    };
    shift(spans, &[edit])
}

/// `spans` of an older text moved on through `edits` (in order) to the text as it is:
/// those before an edit stay, those after it move with the text, and what an edit
/// replaced loses its colors (till they're worked out again).
pub fn shift(spans: &[Span], edits: &[InputEdit]) -> Vec<Span> {
    let mut spans = spans.to_vec();
    for edit in edits {
        spans = shift_one(spans, edit);
    }
    spans
}

fn shift_one(spans: Vec<Span>, edit: &InputEdit) -> Vec<Span> {
    let (start, old_end, new_end) = (edit.start_byte, edit.old_end_byte, edit.new_end_byte);
    // Nothing before the edit changes.
    let first = spans.partition_point(|s| s.range.end <= start);
    if first == spans.len() {
        return spans;
    }
    let mut out = spans;
    let after: Vec<Span> = out.split_off(first);
    let moved = |b: usize| b - old_end + new_end;
    for s in after {
        if s.range.start >= old_end {
            out.push(Span { range: moved(s.range.start)..moved(s.range.end), scope: s.scope });
            continue;
        }
        if s.range.start < start {
            out.push(Span { range: s.range.start..start, scope: s.scope });
        }
        if s.range.end > old_end {
            out.push(Span { range: new_end..moved(s.range.end), scope: s.scope });
        }
    }
    out
}

/// The spans of an older text in `range` of the text as it is, `edits` (in order)
/// since: what [`shift`] makes of them, but only what's there, so it costs as little
/// as what's on screen, however big the file.
pub fn spans_in(spans: &[Span], edits: &[InputEdit], range: Range<usize>) -> Vec<Span> {
    // Where `range` was, before the edits: more, if an edit put in what's at its ends.
    let (mut from, mut to) = (range.start, range.end);
    for edit in edits.iter().rev() {
        let back = |at: usize, end: bool| match () {
            _ if at <= edit.start_byte => at,
            _ if at >= edit.new_end_byte => at - edit.new_end_byte + edit.old_end_byte,
            _ if end => edit.old_end_byte,
            _ => edit.start_byte,
        };
        (from, to) = (back(from, false), back(to, true));
    }
    let first = spans.partition_point(|s| s.range.end <= from);
    let mut out: Vec<Span> = spans[first..].iter().take_while(|s| s.range.start < to).cloned().collect();
    for edit in edits {
        out = shift_one(out, edit);
    }
    out.retain(|s| s.range.end > range.start && s.range.start < range.end);
    out
}

/// `spans` with neighbours of the same scope made one, and none empty.
fn coalesce(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans.into_iter().filter(|s| !s.range.is_empty()) {
        match out.last_mut() {
            Some(last) if last.range.end == span.range.start && last.scope == span.scope => last.range.end = span.range.end,
            _ => out.push(span),
        }
    }
    out
}

/// Reads a node's text from the rope, a piece at a time, for a query's predicates.
pub(crate) struct RopeText<'a>(pub(crate) &'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = std::iter::Map<ropey::iter::Chunks<'a>, fn(&'a str) -> &'a [u8]>;

    fn text(&mut self, node: Node) -> Self::I {
        piece(self.0, node.byte_range()).chunks().map(str::as_bytes as fn(&'a str) -> &'a [u8])
    }
}

/// `range` of `text`, clamped to it and to char boundaries.
fn piece(text: &Rope, range: Range<usize>) -> ropey::RopeSlice<'_> {
    use crate::text::Text as _;
    let (start, end) = (text.floor_boundary(range.start), text.floor_boundary(range.end));
    text.byte_slice(start..end.max(start))
}

/// The spans in `range` of one layer of a file: `tree`, of `grammar`, which is
/// `ranges` of `text` (all of it, or a language's inside another's, `depth` in);
/// and over them, those of the languages inside it.
fn layer(grammar: &Grammar, tree: &Tree, text: &Rope, range: &Range<usize>, ranges: &[tree_sitter::Range], depth: usize) -> Vec<Span> {
    let Ok(loaded) = grammar.loaded() else { return vec![] };
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    // Each capture, in order: where, which node, its scope, and which match it's of.
    let mut found = vec![];
    let mut captures = cursor.captures(&loaded.highlights, tree.root_node(), RopeText(text));
    while let Some((m, i)) = captures.next() {
        let capture = m.captures()[*i];
        found.push((capture.node.byte_range(), capture.node.id(), loaded.scopes[capture.index as usize], m.id()));
    }
    let own = innermost(&found, range);
    let inside = injected_spans(grammar, loaded, tree, text, range, ranges, depth);
    if inside.is_empty() { own } else { coalesce(overlay(&own, &inside)) }
}

/// What a layer's captures color, as tree-sitter's highlighter has it: where several
/// patterns capture one node, the last one's scope (the rest of the matches it beat
/// are dropped); where captures nest, the innermost's. Only what's in `range`.
fn innermost(found: &[(Range<usize>, usize, Option<usize>, u32)], range: &Range<usize>) -> Vec<Span> {
    let mut beaten = HashSet::new();
    let mut out = vec![];
    let mut emit = |from: usize, to: usize, scope: usize| {
        let (from, to) = (from.max(range.start), to.min(range.end));
        if from < to {
            out.push(Span { range: from..to, scope });
        }
    };
    // The captures open, innermost last: where each ends, and its scope.
    let mut open: Vec<(usize, usize)> = vec![];
    let mut at = range.start;
    let mut i = 0;
    while i < found.len() {
        let (bytes, node, mut scope, mut id) = found[i].clone();
        i += 1;
        if beaten.contains(&id) {
            continue;
        }
        loop {
            while found.get(i).is_some_and(|next| beaten.contains(&next.3)) {
                i += 1;
            }
            match found.get(i) {
                Some(next) if next.1 == node => {
                    beaten.insert(id);
                    (scope, id) = (next.2, next.3);
                    i += 1;
                }
                _ => break,
            }
        }
        let Some(scope) = scope else { continue };
        while let Some(&(end, top)) = open.last().filter(|(end, _)| *end <= bytes.start) {
            emit(at, end, top);
            at = at.max(end);
            open.pop();
        }
        if let Some(&(_, top)) = open.last() {
            emit(at, bytes.start, top);
        }
        at = at.max(bytes.start);
        open.push((bytes.end, scope));
    }
    while let Some((end, top)) = open.pop() {
        emit(at, end, top);
        at = at.max(end);
    }
    coalesce(out)
}

/// The spans in `range` of the languages inside a layer (`tree`, of `grammar`, over
/// `ranges`), each parsed on its own, as only what's there.
fn injected_spans(
    grammar: &Grammar,
    loaded: &Loaded,
    tree: &Tree,
    text: &Rope,
    range: &Range<usize>,
    ranges: &[tree_sitter::Range],
    depth: usize,
) -> Vec<Span> {
    let Some(injections) = loaded.injections.as_ref().filter(|_| depth < MAX_DEPTH) else { return vec![] };
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.matches(&injections.query, tree.root_node(), RopeText(text));
    let mut inside = vec![];
    while let Some(m) = matches.next() {
        let Some((name, content, children)) = injection(injections, &grammar.name, m, text) else { continue };
        let Some(inner) = injected(&name) else { continue };
        let ranges = intersect_ranges(ranges, content, children);
        if ranges.is_empty() {
            continue;
        }
        if let Some(tree) = inner.parse_rope(text, None, &ranges) {
            inside.extend(layer(inner, &tree, text, range, &ranges, depth + 1));
        }
    }
    inside.sort_by_key(|s| s.range.start);
    inside
}

/// The language a match of the injection query says is inside, the node it's in, and
/// whether that node's children are in it too (as tree-sitter's highlighter reads it).
fn injection<'t>(injections: &Injections, own: &str, m: &tree_sitter::QueryMatch<'_, 't>, text: &Rope) -> Option<(String, Node<'t>, bool)> {
    let mut name = None;
    let mut content = None;
    for capture in m.captures() {
        if Some(capture.index) == injections.language {
            name = Some(piece(text, capture.node.byte_range()).to_string());
        } else if Some(capture.index) == injections.content {
            content = Some(capture.node);
        }
    }
    let mut children = false;
    for property in injections.query.property_settings(m.pattern_index) {
        match property.key.as_ref() {
            "injection.language" if name.is_none() => name = property.value.as_deref().map(str::to_string),
            "injection.self" if name.is_none() => name = Some(own.to_string()),
            "injection.include-children" => children = true,
            _ => {}
        }
    }
    Some((name?, content?, children))
}

/// What a node is of the text, as a language inside another is parsed: the node, less
/// its children unless they're in it too, and only where the layer it's in is.
/// Tree-sitter's highlighter's `intersect_ranges`, for one node.
fn intersect_ranges(parent: &[tree_sitter::Range], node: Node, children: bool) -> Vec<tree_sitter::Range> {
    let mut result = vec![];
    let mut parents = parent.iter();
    let Some(mut parent) = parents.next() else { return result };
    let mut cursor = node.walk();
    let mut preceding = tree_sitter::Range { start_byte: 0, start_point: Point::new(0, 0), end_byte: node.start_byte(), end_point: node.start_position() };
    let following = tree_sitter::Range {
        start_byte: node.end_byte(),
        start_point: node.end_position(),
        end_byte: usize::MAX,
        end_point: Point::new(usize::MAX, usize::MAX),
    };
    let excluded: Vec<tree_sitter::Range> = node.children(&mut cursor).filter(|_| !children).map(|child| child.range()).chain([following]).collect();
    for excluded in excluded {
        let mut range = tree_sitter::Range {
            start_byte: preceding.end_byte,
            start_point: preceding.end_point,
            end_byte: excluded.start_byte,
            end_point: excluded.start_point,
        };
        preceding = excluded;
        if range.end_byte < parent.start_byte {
            continue;
        }
        while parent.start_byte <= range.end_byte {
            if parent.end_byte > range.start_byte {
                if range.start_byte < parent.start_byte {
                    (range.start_byte, range.start_point) = (parent.start_byte, parent.start_point);
                }
                if parent.end_byte < range.end_byte {
                    if range.start_byte < parent.end_byte {
                        result.push(tree_sitter::Range { end_byte: parent.end_byte, end_point: parent.end_point, ..range });
                    }
                    (range.start_byte, range.start_point) = (parent.end_byte, parent.end_point);
                } else {
                    if range.start_byte < range.end_byte {
                        result.push(range);
                    }
                    break;
                }
            }
            match parents.next() {
                Some(next) => parent = next,
                None => return result,
            }
        }
    }
    result
}

/// What's injected into a language, by its name: a code block's language
/// (by its name, extension or id), or a grammar there only to be injected.
fn injected(name: &str) -> Option<&'static Grammar> {
    let language = Language::all().into_iter().find(|l| l.grammar.is_some() && (l.name == name || l.extensions.contains(&name) || l.id() == name));
    match language {
        Some(language) => language.grammar,
        None => GRAMMARS.read().unwrap_or_else(|e| e.into_inner()).iter().find(|grammar| grammar.name == name).copied(),
    }
}

impl Language {
    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        if let Some(highlighter) = self.highlighter {
            return if text.len() > MAX_HIGHLIGHT_BY_CODE { vec![] } else { highlighter(text) };
        }
        match self.grammar {
            Some(grammar) if text.len() <= MAX_HIGHLIGHT => grammar.highlight(text),
            _ => vec![],
        }
    }
}

/// A file's colors, as worked out for one version of its text, and its syntax
/// tree then, to work out the next version's from. Made and moved on off the
/// UI thread: the text is a snapshot (a rope's clone costs nothing).
#[derive(Clone)]
pub struct Colors {
    pub language: &'static Language,
    /// The buffer's version they're of, and its text then.
    pub version: u64,
    pub text: Rope,
    pub spans: Vec<Span>,
    tree: Option<Tree>,
}

impl std::fmt::Debug for Colors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Colors").field("language", &self.language).field("version", &self.version).field("spans", &self.spans.len()).finish()
    }
}

impl Colors {
    /// `text` (version `version`) in `language`, colored from nothing.
    pub fn new(language: &'static Language, version: u64, text: Rope) -> Self {
        let (tree, spans) = match (language.highlighter, language.grammar) {
            (Some(highlighter), _) if text.len_bytes() <= MAX_HIGHLIGHT_BY_CODE => (None, highlighter(&text.to_string())),
            (None, Some(grammar)) if text.len_bytes() <= MAX_HIGHLIGHT => match grammar.parse_rope(&text, None, &[]) {
                Some(tree) => {
                    let spans = grammar.spans(&tree, &text, 0..text.len_bytes());
                    (Some(tree), spans)
                }
                None => (None, vec![]),
            },
            _ => (None, vec![]),
        };
        Self { language, version, text, spans, tree }
    }

    /// The colors of `text` (version `version`): from `previous`, if there are and
    /// `edits` (all of them since) take them there, or else from nothing.
    pub fn work(language: &'static Language, previous: Option<&Colors>, version: u64, text: Rope, edits: Option<Vec<InputEdit>>) -> Self {
        match (previous.filter(|p| std::ptr::eq(p.language, language)), edits) {
            (Some(previous), Some(edits)) => previous.update(version, text, &edits),
            _ => Self::new(language, version, text),
        }
    }

    /// These colors moved on to `text` (version `version`), which is their text after
    /// `edits`: the tree's edited and parsed again from what it was, and only what
    /// changed in it, and the lines edited (all of a language inside another that
    /// they touch), is colored again; the rest is moved along. Plugin code that
    /// colors does it all again (it can't do part).
    pub fn update(&self, version: u64, text: Rope, edits: &[InputEdit]) -> Self {
        let (Some(old), Some(grammar)) = (&self.tree, self.language.grammar) else { return Self::new(self.language, version, text) };
        if edits.len() > MAX_EDITS || text.len_bytes() > MAX_HIGHLIGHT {
            return Self::new(self.language, version, text);
        }
        let mut edited = old.clone();
        for edit in edits {
            edited.edit(edit);
        }
        let Some(tree) = grammar.parse_rope(&text, Some(&edited), &[]) else { return Self::new(self.language, version, text) };
        let mut dirty: Vec<Range<usize>> = edited.changed_ranges(&tree).map(|r| r.start_byte..r.end_byte).collect();
        // What each edit put in, where it is once the edits after it are made.
        for (i, edit) in edits.iter().enumerate() {
            let mut range = edit.start_byte..edit.new_end_byte;
            for later in &edits[i + 1..] {
                let forward = |at: usize| match () {
                    _ if at <= later.start_byte => at,
                    _ if at >= later.old_end_byte => at - later.old_end_byte + later.new_end_byte,
                    _ => later.new_end_byte,
                };
                range = forward(range.start)..forward(range.end);
            }
            dirty.push(range);
        }
        let dirty = merged(dirty.into_iter().map(|range| whole_lines(&text, range)).collect());
        // A language inside another is all colored again where an edit touched it,
        // as it was and as it is: what's inside it depends on all of it.
        let mut wider = dirty.clone();
        for (tree, text) in [(&edited, &text), (&tree, &text)] {
            for range in &dirty {
                wider.extend(injected_contents(grammar, tree, text, range));
            }
        }
        let dirty = merged(wider.into_iter().map(|range| whole_lines(&text, range)).collect());
        let kept = outside(shift(&self.spans, edits), &dirty);
        let fresh: Vec<Span> = dirty.iter().flat_map(|range| grammar.spans(&tree, &text, range.clone())).collect();
        let spans = coalesce(merge_sorted(kept, fresh));
        Self { language: self.language, version, text, spans, tree: Some(tree) }
    }

    /// Their syntax tree, if their language has a grammar.
    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }
}

/// `range` out to the start of its first line and past the end of its last.
fn whole_lines(text: &Rope, range: Range<usize>) -> Range<usize> {
    let len = text.len_bytes();
    let (start, end) = (range.start.min(len), range.end.min(len));
    let first = text.line_to_byte(text.byte_to_line(start));
    let last = text.byte_to_line(end) + 1;
    first..if last < text.len_lines() { text.line_to_byte(last) } else { len }
}

/// `ranges` in order, those that touch made one.
fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = vec![];
    for range in ranges {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

/// Where what's inside a layer (`tree`, of `grammar`) is, for those of its injections in `range`.
fn injected_contents(grammar: &Grammar, tree: &Tree, text: &Rope, range: &Range<usize>) -> Vec<Range<usize>> {
    let Some(injections) = grammar.loaded().ok().and_then(|loaded| loaded.injections.as_ref()) else { return vec![] };
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.matches(&injections.query, tree.root_node(), RopeText(text));
    let mut found = vec![];
    while let Some(m) = matches.next() {
        found.extend(m.captures().iter().filter(|c| Some(c.index) == injections.content).map(|c| c.node.byte_range()));
    }
    found
}

/// What of `spans` is outside `dirty` (in order, apart).
fn outside(spans: Vec<Span>, dirty: &[Range<usize>]) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let mut from = span.range.start;
        let first = dirty.partition_point(|d| d.end <= from);
        for d in dirty[first..].iter().take_while(|d| d.start < span.range.end) {
            if d.start > from {
                out.push(Span { range: from..d.start, scope: span.scope });
            }
            from = from.max(d.end);
        }
        if from < span.range.end {
            out.push(Span { range: from..span.range.end, scope: span.scope });
        }
    }
    out
}

/// Two lists of spans, each in order and apart from the other's, as one.
fn merge_sorted(a: Vec<Span>, b: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if x.range.start <= y.range.start => a.next(),
            (Some(_), Some(_)) => b.next(),
            (Some(_), None) => a.next(),
            (None, Some(_)) => b.next(),
            (None, None) => break,
        };
        out.extend(next);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The scope at each of `at` in `text`, highlighted as `path`.
    fn scopes_at(path: &str, text: &str, at: &[usize]) -> Vec<Option<&'static str>> {
        crate::language::testing::install();
        let spans = Language::for_path(Path::new(path)).unwrap().highlight(text);
        at.iter().map(|&at| spans.iter().find(|s| s.range.contains(&at)).map(|s| SCOPES[s.scope])).collect()
    }

    #[test]
    fn highlights_rust() {
        let [keyword, name] = scopes_at("src/main.rs", "fn main() { let x = 1; }", &[0, 3])[..] else { unreachable!() };
        assert_eq!(keyword, Some("keyword"));
        assert_eq!(name.map(|s| s.starts_with("function")), Some(true));
    }

    fn spans(list: &[(Range<usize>, usize)]) -> Vec<Span> {
        list.iter().map(|(range, scope)| Span { range: range.clone(), scope: *scope }).collect()
    }

    #[test]
    fn the_servers_spans_win_over_the_grammars() {
        let base = spans(&[(0..10, 1), (12..20, 2)]);
        let top = spans(&[(2..4, 7), (8..14, 8), (25..30, 9)]);
        assert_eq!(overlay(&base, &top), spans(&[(0..2, 1), (2..4, 7), (4..8, 1), (8..14, 8), (14..20, 2), (25..30, 9)]));
        assert_eq!(overlay(&base, &[]), base);
    }

    #[test]
    fn spans_follow_an_edit() {
        let old = "let a = b;";
        let new = "let a = xyz + b;";
        let moved = follow_edit(&spans(&[(0..3, 1), (4..5, 2), (8..9, 3)]), old, new);
        assert_eq!(moved, spans(&[(0..3, 1), (4..5, 2), (14..15, 3)]));
        assert_eq!(follow_edit(&spans(&[(4..5, 2)]), old, "let xb = b;"), spans(&[]), "the edit touched it");
        assert_eq!(follow_edit(&spans(&[(4..5, 2)]), old, "let ab = b;"), spans(&[(4..5, 2)]), "typing on after it leaves it be");
    }

    #[test]
    fn aliases_and_shorter_forms() {
        let name = |capture| scope(capture).map(|s| SCOPES[s]);
        assert_eq!(name("keyword.control.import"), Some("keyword"));
        assert_eq!(name("markup.heading.2"), Some("text.title"));
        assert_eq!(name("constant.numeric.float"), Some("number"));
        assert_eq!(name("function.method.call"), Some("function.method"));
        assert_eq!(name("spell"), None);
    }

    /// A seeded source of numbers, so a failure can be had again.
    struct Seeded(u64);

    impl Seeded {
        fn below(&mut self, n: usize) -> usize {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
        }
    }

    /// Edits as typing makes them, and some it doesn't, here and there in `buffer`.
    fn edit_somewhere(buffer: &mut crate::Buffer, rng: &mut Seeded) {
        const PIECES: &[&str] = &[
            "x",
            " ",
            "\n",
            "{",
            "}",
            "(",
            ")",
            "\"",
            "'",
            "//",
            "/*",
            "*/",
            "fn f() {",
            "let a = 1;",
            "r#\"",
            "#[test]",
            "/// ",
            "é",
            "😀",
            "'a",
            "\\",
            "\n\n",
        ];
        let at = |rng: &mut Seeded, buffer: &crate::Buffer| buffer.next(buffer.prev(crate::Text::floor_boundary(buffer, rng.below(buffer.len() + 1))));
        match rng.below(10) {
            0..=5 => {
                let at = at(rng, buffer);
                buffer.edit(at..at, PIECES[rng.below(PIECES.len())]);
            }
            6..=8 => {
                let from = at(rng, buffer);
                let mut to = from;
                for _ in 0..1 + rng.below(20) {
                    to = buffer.next(to);
                }
                buffer.edit(from..to, "");
            }
            _ => {
                buffer.commit(crate::Range::point(0));
                if rng.below(2) == 0 {
                    buffer.undo()
                } else {
                    buffer.redo()
                };
            }
        }
    }

    /// Colors moved on edit by edit, each time against those of all of it.
    fn incremental_is_whole(language: &'static Language, text: &str, seed: u64, rounds: usize) {
        let mut rng = Seeded(seed);
        let mut buffer = crate::Buffer::new(text);
        let mut colors = Colors::new(language, buffer.version(), buffer.rope().clone());
        assert!(!colors.spans.is_empty());
        for round in 0..rounds {
            for _ in 0..1 + rng.below(4) {
                edit_somewhere(&mut buffer, &mut rng);
            }
            let edits = buffer.changes_since(colors.version).unwrap();
            colors = colors.update(buffer.version(), buffer.rope().clone(), &edits);
            // Of the same tree: one parsed from the last can differ from one parsed from
            // nothing where the text has errors (each recovers from them its own way).
            let tree = colors.tree().unwrap();
            let whole = Colors { spans: language.grammar.unwrap().spans(tree, buffer.rope(), 0..buffer.len()), ..colors.clone() };
            if colors.spans != whole.spans {
                let text = buffer.to_string();
                let wrong = colors.spans.iter().zip(&whole.spans).find(|(a, b)| a != b).map_or(0, |(a, b)| a.range.start.min(b.range.start));
                let around = &text[text.floor_char_boundary(wrong.saturating_sub(60))..text.floor_char_boundary(wrong + 60)];
                let near = |spans: &[Span]| {
                    spans
                        .iter()
                        .filter(|s| s.range.end + 40 > wrong && s.range.start < wrong + 40)
                        .map(|s| (s.range.clone(), SCOPES[s.scope]))
                        .collect::<Vec<_>>()
                };
                panic!("seed {seed}, round {round}: the colors differ at {wrong}, around {around:?}:\n{:?}\n{:?}", near(&colors.spans), near(&whole.spans));
            }
        }
    }

    #[test]
    fn colors_worked_out_bit_by_bit_are_those_of_all_of_it() {
        crate::language::testing::install();
        let rust = Language::for_path(Path::new("a.rs")).unwrap();
        let text = include_str!("textobject.rs");
        for seed in 1..=8 {
            incremental_is_whole(rust, text, seed, 60);
        }
    }

    #[test]
    fn colors_of_a_language_inside_another_are_worked_out_bit_by_bit_too() {
        // Comments that are code, by a grammar registered to be injected.
        let inner = crate::language::testing::grammar("code-in-comments", tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, "", "");
        Grammar::register(inner);
        let injections = r#"((line_comment) @injection.content (#set! injection.language "code-in-comments"))"#;
        let outer = crate::language::testing::grammar("commented", tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, injections, "");
        static COMMENTED: OnceLock<Language> = OnceLock::new();
        let language = COMMENTED.get_or_init(|| Language { name: "commented", grammar: Some(outer), ..Language::DEFAULT });
        let text = "// fn a() { let x = 1; }\nfn b() {}\n// struct S; // \"s\"\n".repeat(30);
        let colors = Colors::new(language, 0, Rope::from_str(&text));
        let keyword = scope_named("keyword");
        assert!(colors.spans.iter().any(|s| s.range.start == 3 && Some(s.scope) == keyword), "the comment's `fn`");
        for seed in 1..=4 {
            incremental_is_whole(language, &text, seed, 40);
        }
    }

    #[test]
    fn colors_shown_meanwhile_are_moved_past_the_edits() {
        crate::language::testing::install();
        let rust = Language::for_path(Path::new("a.rs")).unwrap();
        let mut rng = Seeded(7);
        let mut buffer = crate::Buffer::new(include_str!("selection.rs"));
        let colors = Colors::new(rust, buffer.version(), buffer.rope().clone());
        for _ in 0..40 {
            edit_somewhere(&mut buffer, &mut rng);
            let edits = buffer.changes_since(colors.version).unwrap();
            let all = shift(&colors.spans, &edits);
            let from = rng.below(buffer.len());
            let range = from..(from + rng.below(400)).min(buffer.len());
            let within: Vec<Span> = all.iter().filter(|s| s.range.end > range.start && s.range.start < range.end).cloned().collect();
            assert_eq!(spans_in(&colors.spans, &edits, range), within);
            assert!(all.windows(2).all(|w| w[0].range.end <= w[1].range.start), "in order, apart");
            assert!(all.iter().all(|s| s.range.end <= buffer.len() && buffer.is_char_boundary(s.range.start) && buffer.is_char_boundary(s.range.end)));
        }
    }

    #[test]
    fn a_tree_parsed_from_the_last_one_is_as_one_parsed_from_nothing() {
        crate::language::testing::install();
        let rust = Language::for_path(Path::new("a.rs")).unwrap();
        let mut rng = Seeded(3);
        let mut buffer = crate::Buffer::new(include_str!("movement.rs"));
        for _ in 0..30 {
            edit_somewhere(&mut buffer, &mut rng);
            let kept = crate::nodes::tree(rust, &buffer).unwrap();
            let fresh = rust.grammar.unwrap().parse(&buffer.to_string()).unwrap();
            assert_eq!(kept.root_node().to_sexp(), fresh.root_node().to_sexp());
        }
    }
}
