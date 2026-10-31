//! Lean's unicode input: `\alpha` becomes `α` as it's typed. The
//! abbreviations are the Lean VS Code extension's (leanprover/vscode-lean4,
//! `lean4-unicode-input/src/abbreviations.json`), kept as `abbreviation⇥symbol`
//! lines. `$CURSOR` in a symbol is where the cursor goes (`\<>` is `⟨|⟩`).
//!
//! How typing goes is in [`Modal::insert`](crate::Modal::insert): an
//! abbreviation becomes its symbol as soon as no longer one starts with it,
//! or when something is typed that can't go on to one (a space, say). One
//! that's left unfinished becomes the shortest it starts (`\a ` is `α`).

use std::sync::OnceLock;

/// The abbreviations and their symbols, by abbreviation.
fn table() -> &'static [(&'static str, &'static str)] {
    static TABLE: OnceLock<Vec<(&str, &str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Vec<(&str, &str)> = include_str!("abbreviations.tsv").lines().filter_map(|line| line.split_once('\t')).collect();
        table.sort_unstable();
        table
    })
}

/// The abbreviations that start with `typed`.
fn starting(typed: &str) -> &'static [(&'static str, &'static str)] {
    let table = table();
    let from = table.partition_point(|(k, _)| *k < typed);
    let to = from + table[from..].partition_point(|(k, _)| k.starts_with(typed));
    &table[from..to]
}

/// Whether typing on from `typed` can still make an abbreviation.
pub fn extends(typed: &str) -> bool {
    !starting(typed).is_empty()
}

/// The symbol to put in as soon as `typed` is typed: when it's an
/// abbreviation and no longer one starts with it.
pub fn eager(typed: &str) -> Option<&'static str> {
    match starting(typed) {
        [(k, symbol)] if *k == typed => Some(symbol),
        _ => None,
    }
}

/// The symbol `typed` stands for when it's finished: its own, or the one of
/// the shortest abbreviation it starts.
pub fn finished(typed: &str) -> Option<&'static str> {
    if typed.is_empty() {
        return None;
    }
    starting(typed).iter().min_by_key(|(k, _)| k.len()).map(|(_, symbol)| *symbol)
}

/// `symbol` as it goes in, and where in it the cursor goes.
pub fn place(symbol: &str) -> (String, usize) {
    match symbol.split_once("$CURSOR") {
        Some((before, after)) => (format!("{before}{after}"), before.len()),
        None => (symbol.to_string(), symbol.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviations_stand_for_symbols() {
        assert_eq!(eager("alpha"), Some("α"));
        assert_eq!(eager("a"), None, "alpha starts with it");
        assert_eq!(finished("a"), Some("α"));
        assert_eq!(finished("N"), Some("ℕ"));
        assert_eq!(eager("\\"), Some("\\"), "\\\\ is a backslash");
        assert!(extends("alp") && !extends("alpx"));
        assert_eq!(place("⟨$CURSOR⟩"), ("⟨⟩".to_string(), "⟨".len()));
    }
}
