//! Lean highlighted by its tokens, as its VS Code extension's grammar does:
//! comments, strings, numbers, keywords, tactics, attributes, and the names
//! being declared. (Lean's syntax grows with every `notation`; no grammar
//! knows all of it, only Lean.)

use std::ops::Range;

#[rustfmt::skip]
const KEYWORDS: &[&str] = &[
    "abbrev", "at", "attribute", "axiom", "break", "by", "calc", "catch", "class", "continue", "decreasing_by", "def", "deriving", "do", "elab",
    "else", "end", "example", "export", "extends", "finally", "for", "from", "fun", "have", "if", "import", "in", "include", "inductive", "infix",
    "infixl", "infixr", "instance", "lemma", "let", "local", "macro", "macro_rules", "match", "meta", "mut", "mutual", "namespace", "noncomputable",
    "nomatch", "nofun", "notation", "obtain", "omit", "opaque", "open", "partial", "postfix", "prefix", "private", "protected", "public", "return",
    "scoped", "section", "set_option", "show", "structure", "suffices", "syntax", "termination_by", "then", "theorem", "try", "universe", "unless",
    "unsafe", "variable", "where", "with",
];

/// The keywords a declaration's name follows.
const DECLARING: &[&str] = &["abbrev", "axiom", "class", "def", "inductive", "instance", "lemma", "opaque", "structure", "theorem"];

#[rustfmt::skip]
const TACTICS: &[&str] = &[
    "aesop", "all_goals", "any_goals", "apply", "apply?", "assumption", "by_cases", "by_contra", "case", "cases", "change", "congr", "constructor",
    "contradiction", "conv", "decide", "done", "dsimp", "exact", "exact?", "exfalso", "exists", "ext", "field_simp", "fin_cases", "first", "focus",
    "funext", "gcongr", "grind", "induction", "injection", "interval_cases", "intro", "intros", "left", "linarith", "next", "nlinarith", "norm_cast",
    "norm_num", "omega", "positivity", "push_cast", "push_neg", "rcases", "refine", "repeat", "rewrite", "rfl", "right", "ring", "ring_nf", "rintro",
    "rw", "simp", "simp?", "simp_all", "simpa", "skip", "specialize", "split", "subst", "symm", "tauto", "trivial", "unfold", "use",
];

/// Whether `c` can be in a name (after its first char).
fn naming(c: char) -> bool {
    c.is_alphanumeric() && c != 'λ' || matches!(c, '_' | '\'' | '!' | '?') || ('₀'..='₉').contains(&c)
}

fn starts_name(c: char) -> bool {
    c.is_alphabetic() && !matches!(c, 'λ' | 'Π' | 'Σ') || c == '_' || c == '«'
}

/// Where the block comment from `at` (at its `/-`) ends: after its `-/`,
/// past any nested in it.
fn block_comment_end(text: &str, at: usize) -> usize {
    let mut depth = 0;
    let mut i = at;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("/-") {
            depth += 1;
            i += 2;
        } else if rest.starts_with("-/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    text.len()
}

/// The end of the quoted string starting at `at`, past escapes.
fn string_end(text: &str, at: usize) -> usize {
    let mut chars = text[at + 1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => return at + 1 + i + 1,
            _ => {}
        }
    }
    text.len()
}

pub(crate) fn highlight(text: &str) -> Vec<(Range<usize>, &'static str)> {
    let mut spans = vec![];
    let mut declaring = false;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or(' ');
        let prev = text[..i].chars().next_back();
        let (end, scope) = if rest.starts_with("--") {
            (rest.find('\n').map_or(text.len(), |n| i + n), Some("comment"))
        } else if rest.starts_with("/-") {
            let doc = rest.starts_with("/--") || rest.starts_with("/-!");
            (block_comment_end(text, i), Some(if doc { "comment.doc" } else { "comment" }))
        } else if c == '"' {
            (string_end(text, i), Some("string"))
        } else if c == '\'' && !prev.is_some_and(naming) && char_literal(rest).is_some() {
            (i + char_literal(rest).unwrap_or(1), Some("string.special"))
        } else if rest.starts_with("@[") {
            (rest.find(']').map_or(text.len(), |n| i + n + 1), Some("attribute"))
        } else if c == '#' && rest[1..].starts_with(starts_name) {
            (i + 1 + rest[1..].find(|c: char| !naming(c)).unwrap_or(rest.len() - 1), Some("keyword"))
        } else if c.is_ascii_digit() && !prev.is_some_and(naming) {
            (i + rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_')).unwrap_or(rest.len()), Some("number"))
        } else if starts_name(c) && !prev.is_some_and(naming) {
            let end = name_end(text, i);
            let name = &text[i..end];
            let scope = if KEYWORDS.contains(&name) {
                Some("keyword")
            } else if declaring {
                Some("function")
            } else if TACTICS.contains(&name) {
                Some("function.builtin")
            } else if matches!(name, "Type" | "Prop" | "Sort") {
                Some("type.builtin")
            } else if matches!(name, "true" | "false" | "True" | "False") {
                Some("boolean")
            } else if matches!(name, "sorry" | "admit") {
                Some("constant.builtin")
            } else if name.rsplit('.').next().is_some_and(|last| last.starts_with(char::is_uppercase)) {
                Some("type")
            } else {
                None
            };
            declaring = DECLARING.contains(&name);
            (end, scope)
        } else if matches!(c, '∀' | '∃' | 'λ' | 'Π' | 'Σ' | '⊢') {
            (i + c.len_utf8(), Some("keyword"))
        } else if matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '⟨' | '⟩' | '⦃' | '⦄') {
            (i + c.len_utf8(), Some("punctuation.bracket"))
        } else if matches!(c, ',' | ';') {
            (i + 1, Some("punctuation.delimiter"))
        } else if ":=→←↔<>=|&^+-*/%!~∘×≤≥≠∧∨¬∈∉⊆⊂∪∩▸·$".contains(c) {
            (i + c.len_utf8(), Some("operator"))
        } else {
            (i + c.len_utf8(), None)
        };
        if !c.is_whitespace() && !starts_name(c) {
            declaring = false;
        }
        if let Some(scope) = scope {
            spans.push((i..end, scope));
        }
        i = end.max(i + c.len_utf8());
    }
    spans
}

/// The length of the char literal `rest` starts with (`'a'`, `'\n'`), if it is one.
fn char_literal(rest: &str) -> Option<usize> {
    let mut chars = rest.char_indices().skip(1);
    let (_, c) = chars.next()?;
    if c == '\\' {
        chars.next()?;
    }
    let (i, close) = chars.next()?;
    (close == '\'' && c != '\'').then_some(i + 1)
}

/// The end of the name at `at`: dotted parts and all (`Nat.succ_le`), or a `«quoted name»`.
fn name_end(text: &str, at: usize) -> usize {
    let mut i = at;
    loop {
        let rest = &text[i..];
        i += if rest.starts_with('«') {
            rest.find('»').map_or(rest.len(), |n| n + '»'.len_utf8())
        } else {
            rest.find(|c: char| !naming(c)).unwrap_or(rest.len())
        };
        let after = &text[i..];
        match after.strip_prefix('.') {
            Some(next) if next.starts_with(starts_name) => i += 1,
            _ => return i,
        }
    }
}
