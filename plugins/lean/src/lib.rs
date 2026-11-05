//! Lean: its colors, by its tokens, and its goals beside a file, as its infoview
//! shows them. The rest is its `plugin.json`: `lake serve` (so the project's
//! toolchain and packages are used), and typing with backslashes (`\alpha` is `α`,
//! as in its VS Code extension, whose table `abbreviations.tsv` is:
//! `leanprover/vscode-lean4`, `lean4-unicode-input/src/abbreviations.json`).

mod highlight;

use ghostwrite_plugin_api::*;
use serde_json::{Value, json};

/// What Lean says about the cursor, as its infoview shows it: the proof state
/// there (`$/lean/plainGoal`), or else the type expected (`$/lean/plainTermGoal`);
/// and how far it's got checking the file (`$/lean/fileProgress`). Kept, between
/// calls, as the panel's state.
#[derive(Default)]
struct Goals {
    /// The goals at the cursor, when it's in a proof (none left: it's done).
    goals: Option<Vec<String>>,
    /// The type expected at the cursor, when it's in a term.
    expected: Option<String>,
    /// The first line Lean is still checking.
    checking: Option<u32>,
}

impl Goals {
    fn kept(state: &str) -> Self {
        let kept: Value = serde_json::from_str(state).unwrap_or(Value::Null);
        Self {
            goals: kept["goals"].as_array().map(|list| list.iter().filter_map(|g| Some(g.as_str()?.to_string())).collect()),
            expected: kept["expected"].as_str().map(str::to_string),
            checking: kept["checking"].as_u64().map(|line| line as u32),
        }
    }

    fn shown(&self) -> Panel {
        let state = json!({ "goals": self.goals, "expected": self.expected, "checking": self.checking }).to_string();
        Panel { state, blocks: self.blocks(), working_on: self.checking }
    }

    fn answered(&mut self, answers: Vec<Option<Value>>) {
        let mut answers = answers.into_iter();
        let (proof, term) = (answers.next().flatten(), answers.next().flatten());
        self.goals = proof.map(|p| p["goals"].as_array().into_iter().flatten().filter_map(|g| Some(g.as_str()?.to_string())).collect());
        self.expected = term.and_then(|t| Some(t["goal"].as_str()?.to_string()));
    }

    /// Its progress: once it's done, the goals are asked for again.
    fn notification(&mut self, method: &str, params: &Value) -> bool {
        if method != "$/lean/fileProgress" {
            return false;
        }
        let processing = params["processing"].as_array().into_iter().flatten();
        let checking = processing.filter_map(|p| p["range"]["start"]["line"].as_u64()).min().map(|line| line as u32);
        let finished = checking.is_none() && self.checking.is_some();
        self.checking = checking;
        finished
    }

    fn blocks(&self) -> Vec<Block> {
        let code = |text: &String| Block::Code(CodeBlock { text: text.clone(), stands_out: Some('⊢') });
        match (&self.goals, &self.expected) {
            (Some(list), _) if list.is_empty() => vec![Block::Good("No goals".into())],
            (Some(list), _) => {
                let heading = if list.len() == 1 { "1 goal".to_string() } else { format!("{} goals", list.len()) };
                std::iter::once(Block::Heading(heading)).chain(list.iter().map(code)).collect()
            }
            (None, Some(expected)) => vec![Block::Heading("Expected type".into()), code(expected)],
            (None, None) => vec![Block::Muted("No goals here: put the cursor in a proof".into())],
        }
    }
}

struct Lean;

impl Plugin for Lean {
    fn highlight(text: String) -> Vec<Span> {
        let spans = highlight::highlight(&text).into_iter();
        spans.map(|(range, scope)| Span { start: range.start as u32, end: range.end as u32, scope: scope.into() }).collect()
    }

    fn panel_open(_language: String) -> Panel {
        Goals::default().shown()
    }

    fn panel_answered(state: String, answers: Vec<Option<String>>) -> Panel {
        let mut goals = Goals::kept(&state);
        goals.answered(answers.into_iter().map(|answer| serde_json::from_str(&answer?).ok()).collect());
        goals.shown()
    }

    fn panel_notification(state: String, method: String, params: String) -> (Panel, bool) {
        let mut goals = Goals::kept(&state);
        let again = goals.notification(&method, &serde_json::from_str(&params).unwrap_or(Value::Null));
        (goals.shown(), again)
    }
}

register!(Lean);
