//! Indentation: detecting a file's indent unit, and shifting lines.

/// One level of indent, as a file uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndentUnit {
    pub width: usize,
    pub hard_tabs: bool,
}

impl Default for IndentUnit {
    fn default() -> Self {
        Self { width: 4, hard_tabs: false }
    }
}

impl IndentUnit {
    /// Like helix's auto-detection: count the steps between consecutive
    /// lines' indents and take the most common of 2, 4 or 8. A file that's
    /// mostly tab-indented keeps tabs; with nothing to go on it's 4 spaces.
    pub fn detect(text: &str) -> Self {
        let (mut tabs, mut spaced, mut steps) = (0, 0, [0usize; 9]);
        let mut prev = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if line.starts_with('\t') {
                tabs += 1;
                continue;
            }
            let indent = line.len() - line.trim_start_matches(' ').len();
            if indent > 0 {
                spaced += 1;
            }
            let step = indent.abs_diff(prev);
            if (1..=8).contains(&step) {
                steps[step] += 1;
            }
            prev = indent;
        }
        if tabs > spaced {
            return Self { width: 4, hard_tabs: true };
        }
        let width = [4, 2, 8].into_iter().max_by_key(|&w| (steps[w], w == 4)).filter(|&w| steps[w] > 0).unwrap_or(4);
        Self { width, hard_tabs: false }
    }

    pub fn as_str(&self) -> String {
        if self.hard_tabs { "\t".into() } else { " ".repeat(self.width) }
    }
}

/// `line` with `count` levels of indent taken off.
pub fn unindent(line: &str, count: usize, width: usize) -> &str {
    let mut rest = line;
    for _ in 0..count {
        rest = match rest.strip_prefix('\t') {
            Some(r) => r,
            None => {
                let spaces = rest.len() - rest.trim_start_matches(' ').len();
                &rest[spaces.min(width)..]
            }
        };
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_indent() {
        let detect = |t: &str| {
            let u = IndentUnit::detect(t);
            (u.width, u.hard_tabs)
        };
        assert_eq!(detect("def f():\n    x = 1\n    if x:\n        y\n"), (4, false));
        assert_eq!(detect("{\n  a: 1,\n  b: {\n    c\n  }\n}\n"), (2, false));
        assert_eq!(detect("fn f() {\n\tx();\n}\n"), (4, true));
        assert_eq!(detect("no indent here\n"), (4, false));
    }

    #[test]
    fn unindents_tabs_and_spaces() {
        assert_eq!(unindent("        x", 1, 4), "    x");
        assert_eq!(unindent("\t\tx", 2, 4), "x");
        assert_eq!(unindent("  x", 1, 4), "x");
    }
}
