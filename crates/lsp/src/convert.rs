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
            let (range, new_text) = match item.text_edit {
                Some(lsp_types::CompletionTextEdit::Edit(edit)) => (range(text, edit.range), edit.new_text),
                Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => (range(text, edit.replace), edit.new_text),
                None => (typed_from..offset, item.insert_text.clone().unwrap_or_else(|| item.label.clone())),
            };
            let key = item.filter_text.as_deref().unwrap_or(&item.label).to_lowercase();
            let prefix = key.starts_with(&typed);
            (prefix || key.contains(&typed)).then(|| {
                let sort = item.sort_text.unwrap_or_else(|| item.label.clone());
                (prefix, sort, Completion { label: item.label, detail: item.detail, range, text: new_text })
            })
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.into_iter().map(|(_, _, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
