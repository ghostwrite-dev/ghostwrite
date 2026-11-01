//! Unicode input by abbreviation, for a language that has a table of them
//! ([`Language::abbreviations`](crate::Language::abbreviations)): `\alpha`
//! becomes `α` as it's typed.
//! `$CURSOR` in a symbol is where the cursor goes (`\<>` is `⟨|⟩`).
//!
//! How typing goes is in [`Modal::insert`](crate::Modal::insert): an
//! abbreviation becomes its symbol as soon as no longer one starts with it,
//! or when something is typed that can't go on to one (a space, say). One
//! that's left unfinished becomes the shortest it starts (`\a ` is `α`).

use std::sync::OnceLock;

/// A language's abbreviations and their symbols, as `abbreviation⇥symbol`
/// lines, read on first use.
pub struct Abbreviations {
    source: &'static str,
    table: OnceLock<Vec<(&'static str, &'static str)>>,
}

impl Abbreviations {
    pub const fn new(source: &'static str) -> Self {
        Self { source, table: OnceLock::new() }
    }

    /// By abbreviation.
    fn table(&self) -> &[(&'static str, &'static str)] {
        self.table.get_or_init(|| {
            let mut table: Vec<(&str, &str)> = self.source.lines().filter_map(|line| line.split_once('\t')).collect();
            table.sort_unstable();
            table
        })
    }

    /// The abbreviations that start with `typed`.
    fn starting(&self, typed: &str) -> &[(&'static str, &'static str)] {
        let table = self.table();
        let from = table.partition_point(|(k, _)| *k < typed);
        let to = from + table[from..].partition_point(|(k, _)| k.starts_with(typed));
        &table[from..to]
    }

    /// Whether typing on from `typed` can still make an abbreviation.
    pub fn extends(&self, typed: &str) -> bool {
        !self.starting(typed).is_empty()
    }

    /// The symbol to put in as soon as `typed` is typed: when it's an
    /// abbreviation and no longer one starts with it.
    pub fn eager(&self, typed: &str) -> Option<&'static str> {
        match self.starting(typed) {
            [(k, symbol)] if *k == typed => Some(symbol),
            _ => None,
        }
    }

    /// The symbol `typed` stands for when it's finished: its own, or the one of
    /// the shortest abbreviation it starts.
    pub fn finished(&self, typed: &str) -> Option<&'static str> {
        if typed.is_empty() {
            return None;
        }
        self.starting(typed).iter().min_by_key(|(k, _)| k.len()).map(|(_, symbol)| *symbol)
    }
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

    static TABLE: Abbreviations = Abbreviations::new("a\tα\nalpha\tα\nN\tℕ\n\\\t\\\n");

    #[test]
    fn abbreviations_stand_for_symbols() {
        assert_eq!(TABLE.eager("alpha"), Some("α"));
        assert_eq!(TABLE.eager("a"), None, "alpha starts with it");
        assert_eq!(TABLE.finished("a"), Some("α"));
        assert_eq!(TABLE.finished("N"), Some("ℕ"));
        assert_eq!(TABLE.eager("\\"), Some("\\"), "\\\\ is a backslash");
        assert!(TABLE.extends("alp") && !TABLE.extends("alpx"));
        assert_eq!(place("⟨$CURSOR⟩"), ("⟨⟩".to_string(), "⟨".len()));
    }
}
