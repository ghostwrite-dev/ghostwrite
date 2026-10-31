//! Between the protocol and the editor: URIs and paths, UTF-16 positions and
//! byte offsets, and server answers as plain values.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{Completion, Diagnostic, Severity};

pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn path_of(uri: &str) -> Option<PathBuf> {
    Some(PathBuf::from(percent_decode(uri.strip_prefix("file://")?)))
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Byte offset → LSP position (line, UTF-16 column).
pub fn position(text: &str, offset: usize) -> lsp_types::Position {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..line_start].matches('\n').count();
    let character: usize = text[line_start..offset].chars().map(char::len_utf16).sum();
    lsp_types::Position::new(line as u32, character as u32)
}

/// LSP position → byte offset, clamped to the line.
pub fn offset(text: &str, pos: lsp_types::Position) -> usize {
    let line_start = if pos.line == 0 { 0 } else { text.match_indices('\n').nth(pos.line as usize - 1).map_or(text.len(), |(i, _)| i + 1) };
    let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
    let mut units = 0;
    for (i, c) in text[line_start..line_end].char_indices() {
        if units >= pos.character as usize {
            return line_start + i;
        }
        units += c.len_utf16();
    }
    line_end
}

/// The token types we ask servers for, standard and common: what each colors as is [`token_scope`].
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];

/// The scope a semantic token shows as (one of `ghostwrite_core::syntax`'s);
/// none for what a server marks but we leave to the grammar (punctuation, say).
pub fn token_scope(kind: &str, builtin: bool) -> Option<&'static str> {
    Some(match kind {
        "namespace" => "module",
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "typeAlias" if builtin => "type.builtin",
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "typeAlias" => "type",
        "builtinType" => "type.builtin",
        "parameter" => "variable.parameter",
        "variable" if builtin => "variable.builtin",
        "variable" => "variable",
        "property" | "field" | "event" => "property",
        "enumMember" => "constructor",
        "function" if builtin => "function.builtin",
        "function" => "function",
        "method" => "function.method",
        "macro" => "function.macro",
        "keyword" | "modifier" => "keyword",
        "selfKeyword" | "selfTypeKeyword" => "variable.builtin",
        "comment" => "comment",
        "string" => "string",
        "number" => "number",
        "boolean" => "boolean",
        "regexp" => "string.regex",
        "escapeSequence" => "escape",
        "formatSpecifier" => "string.special",
        "operator" => "operator",
        "decorator" | "attribute" => "attribute",
        "lifetime" => "label",
        // Lean's: `sorry`, `admit`.
        "leanSorryLike" => "constant.builtin",
        _ => return None,
    })
}

/// A semantic tokens answer's id and its tokens, encoded: all of them, or
/// (from `…/full/delta`) `previous` with the answer's edits made.
pub fn token_data(value: &Value, previous: Option<&[u32]>) -> Option<(Option<String>, Vec<u32>)> {
    let numbers = |v: &Value| v.as_array().map(|a| a.iter().map(|n| n.as_u64().unwrap_or(0) as u32).collect::<Vec<u32>>());
    let id = value["resultId"].as_str().map(str::to_string);
    if let Some(data) = numbers(&value["data"]) {
        return Some((id, data));
    }
    let mut data = previous?.to_vec();
    let mut edits: Vec<&Value> = value["edits"].as_array()?.iter().collect();
    // From the last, so each edit's start is still where it was.
    edits.sort_by_key(|e| std::cmp::Reverse(e["start"].as_u64()));
    for edit in edits {
        let start = (edit["start"].as_u64()? as usize).min(data.len());
        let end = (start + edit["deleteCount"].as_u64().unwrap_or(0) as usize).min(data.len());
        data.splice(start..end, numbers(&edit["data"]).unwrap_or_default());
    }
    Some((id, data))
}

/// Semantic tokens (as [`token_data`] has them), for `text`, as byte ranges
/// and the scopes they show as (see [`token_scope`]), in order.
pub fn semantic_tokens(data: &[u32], text: &str, (types, modifiers): &(Vec<String>, Vec<String>)) -> Vec<(std::ops::Range<usize>, &'static str)> {
    let builtin = modifiers.iter().position(|m| m == "defaultLibrary").map_or(0, |i| 1u32 << i);
    let lines: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    // A UTF-16 column on a line, as a byte offset.
    let at = |line: usize, column: u32| {
        let start = lines.get(line).copied().unwrap_or(text.len());
        let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        let mut units = 0;
        for (i, c) in text[start..end].char_indices() {
            if units >= column as usize {
                return start + i;
            }
            units += c.len_utf16();
        }
        end
    };
    let (mut line, mut column) = (0, 0);
    let mut out = vec![];
    for token in data.chunks_exact(5) {
        let [delta_line, delta_start, length, kind, mods] = token else { continue };
        if *delta_line > 0 {
            (line, column) = (line + *delta_line as usize, *delta_start);
        } else {
            column += delta_start;
        }
        let Some(scope) = types.get(*kind as usize).and_then(|kind| token_scope(kind, mods & builtin != 0)) else { continue };
        let start = at(line, column);
        out.push((start..at(line, column + length).max(start), scope));
    }
    out
}

pub fn range(text: &str, r: lsp_types::Range) -> std::ops::Range<usize> {
    let (start, end) = (offset(text, r.start), offset(text, r.end));
    start..end.max(start)
}

pub fn diagnostics(text: &str, diagnostics: Vec<lsp_types::Diagnostic>) -> Vec<Diagnostic> {
    use lsp_types::DiagnosticSeverity as S;
    let mut out: Vec<Diagnostic> = diagnostics
        .into_iter()
        .map(|d| Diagnostic {
            range: range(text, d.range),
            severity: match d.severity {
                Some(S::ERROR) | None => Severity::Error,
                Some(S::WARNING) => Severity::Warning,
                Some(S::INFORMATION) => Severity::Info,
                Some(_) => Severity::Hint,
            },
            message: d.message,
        })
        .collect();
    out.sort_by_key(|d| (d.range.start, d.severity));
    out
}

/// Hover contents as plain text, code fences turned into indented blocks.
pub fn hover_text(value: &Value) -> Option<String> {
    let raw = match &value["contents"] {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or_default().to_string(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| p.as_str().map(str::to_string).or_else(|| p["value"].as_str().map(str::to_string)).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => return None,
    };
    let mut out = vec![];
    let mut in_code = false;
    for line in raw.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        out.push(if in_code { format!("    {line}") } else { line.to_string() });
    }
    let text = out.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The first location in a definition answer: a path and its position.
pub fn first_location(value: &Value) -> Option<(PathBuf, lsp_types::Position)> {
    let one = if value.is_array() { value.get(0)? } else { value };
    let uri = one["uri"].as_str().or_else(|| one["targetUri"].as_str())?;
    let range = if one["targetSelectionRange"].is_object() { &one["targetSelectionRange"] } else { &one["range"] };
    let start = serde_json::from_value(range["start"].clone()).ok()?;
    Some((path_of(uri)?, start))
}

/// Where the word being typed at `offset` starts.
pub fn word_start(text: &str, offset: usize) -> usize {
    text[..offset].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(offset, |(i, _)| i)
}

/// A completion answer as edits on `text`, typed at `offset`: ones that
/// start with what's typed first, then the rest that contain it.
pub fn completions(value: Value, text: &str, offset: usize) -> Vec<Completion> {
    let items: Vec<lsp_types::CompletionItem> = match serde_json::from_value::<lsp_types::CompletionResponse>(value) {
        Ok(lsp_types::CompletionResponse::Array(items)) => items,
        Ok(lsp_types::CompletionResponse::List(list)) => list.items,
        Err(_) => return vec![],
    };
    let typed_from = word_start(text, offset);
    let typed = text[typed_from..offset].to_lowercase();
    let mut out: Vec<(bool, String, Completion)> = items
        .into_iter()
        .filter_map(|item| {
            let more = item.additional_text_edits.iter().flatten().map(|e| (range(text, e.range), e.new_text.clone())).collect();
            let (range, new_text) = match item.text_edit {
                Some(lsp_types::CompletionTextEdit::Edit(edit)) => (range(text, edit.range), edit.new_text),
                Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => (range(text, edit.replace), edit.new_text),
                None => (typed_from..offset, item.insert_text.clone().unwrap_or_else(|| item.label.clone())),
            };
            let key = item.filter_text.as_deref().unwrap_or(&item.label).to_lowercase();
            let prefix = key.starts_with(&typed);
            (prefix || key.contains(&typed)).then(|| {
                let sort = item.sort_text.unwrap_or_else(|| item.label.clone());
                (prefix, sort, Completion { label: item.label, detail: item.detail, range, text: new_text, more })
            })
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.into_iter().map(|(_, _, c)| c).collect()
}

/// Text edits (formatting, a completion's imports) as the server sent them.
pub fn text_edits(value: &Value) -> Vec<crate::TextEdit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| Some((serde_json::from_value(e["range"].clone()).ok()?, e["newText"].as_str()?.to_string())))
        .collect()
}

/// `edits` made to `text` (each in the text as it was, as the protocol has
/// them): from the last, so each one's place is still where it was.
pub fn apply(text: &str, edits: &[crate::TextEdit]) -> Vec<(std::ops::Range<usize>, String)> {
    let mut placed: Vec<(std::ops::Range<usize>, String)> = edits.iter().map(|(r, t)| (range(text, *r), t.clone())).collect();
    placed.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    placed
}

/// A workspace edit (rename, a code action): each file's edits.
pub fn workspace_edit(value: &Value) -> Vec<(PathBuf, Vec<crate::TextEdit>)> {
    let mut out = vec![];
    if let Some(changes) = value["changes"].as_object() {
        out.extend(changes.iter().filter_map(|(uri, edits)| Some((path_of(uri)?, text_edits(edits)))));
    }
    for change in value["documentChanges"].as_array().into_iter().flatten() {
        // Creating, renaming and deleting files aren't done: only edits.
        if let Some(path) = change["textDocument"]["uri"].as_str().and_then(path_of) {
            out.push((path, text_edits(&change["edits"])));
        }
    }
    out
}

/// Locations (references, definitions): each file and where in it.
pub fn locations(value: &Value) -> Vec<(PathBuf, lsp_types::Range)> {
    let one = |v: &Value| -> Option<(PathBuf, lsp_types::Range)> {
        let uri = v["uri"].as_str().or(v["targetUri"].as_str())?;
        let range = if v["range"].is_null() { &v["targetSelectionRange"] } else { &v["range"] };
        Some((path_of(uri)?, serde_json::from_value(range.clone()).ok()?))
    };
    match value {
        Value::Array(all) => all.iter().filter_map(one).collect(),
        single => one(single).into_iter().collect(),
    }
}

/// A file's symbols (`textDocument/documentSymbol`), nested ones with their containers.
pub fn symbols(value: &Value) -> Vec<crate::Symbol> {
    const KINDS: [&str; 27] = [
        "",
        "file",
        "module",
        "namespace",
        "package",
        "class",
        "method",
        "property",
        "field",
        "constructor",
        "enum",
        "interface",
        "function",
        "variable",
        "constant",
        "string",
        "number",
        "boolean",
        "array",
        "object",
        "key",
        "null",
        "variant",
        "struct",
        "event",
        "operator",
        "type parameter",
    ];
    fn walk(items: &Value, within: &[String], out: &mut Vec<crate::Symbol>) {
        for item in items.as_array().into_iter().flatten() {
            let Some(name) = item["name"].as_str() else { continue };
            let kind = KINDS.get(item["kind"].as_u64().unwrap_or(0) as usize).copied().unwrap_or("");
            // Hierarchical (`selectionRange`), or flat (`location`).
            let range = if item["selectionRange"].is_null() { &item["location"]["range"] } else { &item["selectionRange"] };
            let Ok(range) = serde_json::from_value(range.clone()) else { continue };
            out.push(crate::Symbol { name: name.to_string(), kind, within: within.to_vec(), range });
            let mut deeper = within.to_vec();
            deeper.push(name.to_string());
            walk(&item["children"], &deeper, out);
        }
    }
    let mut out = vec![];
    walk(value, &[], &mut out);
    out
}

/// The code actions offered (`textDocument/codeAction`).
pub fn code_actions(value: &Value) -> Vec<crate::CodeAction> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["disabled"].is_null())
        .filter_map(|a| {
            let title = a["title"].as_str()?.to_string();
            // A bare command is an action too.
            let command = if a["command"].is_string() { Some(a.clone()) } else { a.get("command").cloned() };
            Some(crate::CodeAction { title, edit: a.get("edit").cloned(), command, raw: a.clone() })
        })
        .collect()
}

/// The signature being typed (`textDocument/signatureHelp`): its label, and
/// which bytes of it are the parameter at the cursor.
pub fn signature(value: &Value) -> Option<(String, Option<std::ops::Range<usize>>)> {
    let signatures = value["signatures"].as_array()?;
    let active = value["activeSignature"].as_u64().unwrap_or(0) as usize;
    let signature = signatures.get(active).or(signatures.first())?;
    let label = signature["label"].as_str()?.to_string();
    let parameter = signature["activeParameter"].as_u64().or(value["activeParameter"].as_u64()).unwrap_or(0) as usize;
    let range = signature["parameters"].get(parameter).and_then(|p| match &p["label"] {
        Value::String(s) => label.find(s.as_str()).map(|at| at..at + s.len()),
        // UTF-16 offsets into the label.
        Value::Array(a) => {
            let unit = |n: &Value| -> Option<usize> {
                let n = n.as_u64()? as usize;
                let mut units = 0;
                for (i, c) in label.char_indices() {
                    if units >= n {
                        return Some(i);
                    }
                    units += c.len_utf16();
                }
                Some(label.len())
            };
            Some(unit(a.first()?)?..unit(a.get(1)?)?)
        }
        _ => None,
    });
    Some((label, range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_tokens_are_byte_ranges_with_scopes() {
        let text = "let α = f(x);\nfoo";
        let legend = (vec!["variable".into(), "function".into(), "punctuation".into()], vec!["declaration".into(), "defaultLibrary".into()]);
        // α (UTF-16 length 1), f (from the default library), `(` unmapped, foo on the next line.
        let value = serde_json::json!({ "resultId": "1", "data": [0, 4, 1, 0, 0, 0, 4, 1, 1, 2, 0, 1, 1, 2, 0, 1, 0, 3, 0, 0] });
        let (id, data) = token_data(&value, None).unwrap();
        assert_eq!(id.as_deref(), Some("1"));
        let tokens = semantic_tokens(&data, text, &legend);
        assert_eq!(tokens, [(4..6, "variable"), (9..10, "function.builtin"), (15..18, "variable")]);

        // Only what changed: foo becomes a function, and the `(` goes.
        let delta = serde_json::json!({ "resultId": "2", "edits": [{ "start": 18, "deleteCount": 1, "data": [1] }, { "start": 10, "deleteCount": 5 }] });
        let (id, data) = token_data(&delta, Some(&data)).unwrap();
        assert_eq!(id.as_deref(), Some("2"));
        assert_eq!(semantic_tokens(&data, text, &legend), [(4..6, "variable"), (9..10, "function.builtin"), (15..18, "function")]);
        assert!(token_data(&delta, None).is_none(), "edits need what they edit");
    }
    #[test]
    fn workspace_edits_symbols_and_signatures() {
        let edit = serde_json::json!({
            "changes": { "file:///a.rs": [{ "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 6 } }, "newText": "bar" }] },
            "documentChanges": [{ "textDocument": { "uri": "file:///b.rs", "version": 1 }, "edits": [] }]
        });
        let files = workspace_edit(&edit);
        assert_eq!(files.len(), 2);
        assert_eq!(apply("fn foo()", &files[0].1), [(3..6, "bar".to_string())]);

        let nested = serde_json::json!([{ "name": "Editor", "kind": 23, "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 6 } },
            "children": [{ "name": "new", "kind": 6, "selectionRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 3 } } }] }]);
        let found = symbols(&nested);
        assert_eq!((found[1].name.as_str(), found[1].kind, found[1].within.clone()), ("new", "method", vec!["Editor".to_string()]));

        let help = serde_json::json!({ "signatures": [{ "label": "fn add(a: i32, b: i32)", "parameters": [{ "label": "a: i32" }, { "label": [15, 21] }] }], "activeParameter": 1 });
        assert_eq!(signature(&help), Some(("fn add(a: i32, b: i32)".to_string(), Some(15..21))));
    }

    use serde_json::json;

    #[test]
    fn positions_count_utf16() {
        let text = "ab\nä😀x\n";
        // 'x' is after ä (1 unit) and 😀 (2 units).
        let x = text.find('x').unwrap();
        assert_eq!(position(text, x), lsp_types::Position::new(1, 3));
        assert_eq!(offset(text, lsp_types::Position::new(1, 3)), x);
        assert_eq!(offset(text, lsp_types::Position::new(1, 99)), text.len() - 1, "clamped to the line");
        assert_eq!(offset(text, lsp_types::Position::new(0, 1)), 1);
    }

    #[test]
    fn uris_round_trip() {
        let p = Path::new("/tmp/my dir/a#b.rs");
        assert_eq!(uri(p), "file:///tmp/my%20dir/a%23b.rs");
        assert_eq!(path_of(&uri(p)).unwrap(), p);
    }

    #[test]
    fn completions_replace_the_typed_word() {
        let text = "let x = foo.ba";
        let answer = json!([{ "label": "bar", "sortText": "2" }, { "label": "abacus", "sortText": "1" }, { "label": "zzz" }]);
        let got = completions(answer, text, text.len());
        assert_eq!(got.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["bar", "abacus"], "prefix matches first");
        assert_eq!(got[0].range, 12..14);
    }

    #[test]
    fn hover_code_becomes_indented() {
        let hover = json!({ "contents": { "kind": "markdown", "value": "```rust\nfn f()\n```\nDoes f." } });
        assert_eq!(hover_text(&hover).unwrap(), "fn f()\nDoes f.");
    }
}
