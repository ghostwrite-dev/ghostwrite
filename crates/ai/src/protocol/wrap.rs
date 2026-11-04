//! Wrapping prose and code to a width, for showing them.

/// Word-wrap prose to `cols` columns, keeping its blank lines and indentation.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(20);
    let mut rows = vec![];
    for line in text.trim_end().split('\n') {
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let mut row = indent.clone();
        for word in line.split_whitespace() {
            let len = row.chars().count();
            if len > indent.len() && len + 1 + word.chars().count() > cols {
                rows.push(std::mem::replace(&mut row, indent.clone()));
            }
            if row.chars().count() > indent.len() {
                row.push(' ');
            }
            row.push_str(word);
        }
        rows.push(row);
    }
    rows
}

/// A code line wrapped to `cols`: at a space where there is one, never
/// touching the spacing inside it; continuations indent one level deeper.
pub fn wrap_code(line: &str, cols: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= cols {
        return vec![line.to_string()];
    }
    let indent = chars.iter().take_while(|c| **c == ' ').count();
    let more = " ".repeat(indent + 4);
    let mut rows = vec![];
    let mut rest: &[char] = &chars;
    let mut prefix = String::new();
    while prefix.chars().count() + rest.len() > cols {
        let room = cols.saturating_sub(prefix.chars().count()).max(8).min(rest.len());
        let lead = if rows.is_empty() { indent } else { 0 };
        let cut = rest[..room].iter().rposition(|c| *c == ' ').filter(|&i| i > lead).map_or(room, |i| i + 1);
        rows.push(format!("{prefix}{}", rest[..cut].iter().collect::<String>().trim_end()));
        rest = &rest[cut..];
        prefix.clone_from(&more);
    }
    rows.push(format!("{prefix}{}", rest.iter().collect::<String>()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_prose_and_code() {
        assert_eq!(wrap("one two three four", 20), ["one two three four"]);
        assert_eq!(wrap("aaaa bbbb cccc dddd eeee ffff\n\n    code(x)", 20), ["aaaa bbbb cccc dddd", "eeee ffff", "", "    code(x)"]);
        assert_eq!(wrap_code("    short", 40), ["    short"]);
        let rows = wrap_code("    result = compute(alpha, beta, gamma, delta)", 28);
        assert_eq!(rows, ["    result = compute(alpha,", "        beta, gamma, delta)"]);
        assert_eq!(wrap_code("abcdefghijklmnopqrstuvwxyz0123456789", 30).len(), 2);
    }
}
