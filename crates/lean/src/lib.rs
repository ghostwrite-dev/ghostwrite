//! Lean, as a plugin: its language, highlighted by its tokens and typed with
//! backslashes (`\alpha` is `α`, as in its VS Code extension, whose table this
//! is), `lake serve` (so the project's toolchain and packages are used), and its
//! goals beside a file, as its infoview shows them.

mod highlight;

use ghostwrite_core::Language;
use ghostwrite_core::abbreviation::Abbreviations;
use ghostwrite_lsp::registry::Spec;
use ghostwrite_plugin::{Block, Panel};
use serde_json::Value;

/// `leanprover/vscode-lean4`, `lean4-unicode-input/src/abbreviations.json`.
static ABBREVIATIONS: Abbreviations = Abbreviations::new(include_str!("abbreviations.tsv"));

static LEAN: Language = Language {
    name: "lean",
    comment: Some("--"),
    extensions: &["lean"],
    servers: &["lean"],
    // No tree-sitter grammar for Lean fits; it's lexed, as its VS Code extension does.
    lex: Some(highlight::highlight),
    abbreviations: Some(&ABBREVIATIONS),
    // A name is quoted with one: `` `name ``.
    unpaired: &['`'],
    ..Language::DEFAULT
};

// Lake picks the project's Lean (elan reads its lean-toolchain) and its packages.
static SERVER: Spec = Spec { name: "lean", command: &["lake", "serve"], roots: &["lakefile.lean", "lakefile.toml", "lean-toolchain"], ..Spec::DEFAULT };

pub struct Plugin;

impl ghostwrite_plugin::Plugin for Plugin {
    fn keys(&self, language: Option<&Language>) -> &'static [ghostwrite_plugin::Binding] {
        const KEYS: &[ghostwrite_plugin::Binding] = &[ghostwrite_plugin::Binding { keys: "space v", command: "preview", what: "show or hide the goals" }];
        if language.is_some_and(|l| std::ptr::eq(l, &LEAN)) { KEYS } else { &[] }
    }

    fn languages(&self) -> Vec<&'static Language> {
        vec![&LEAN]
    }

    fn servers(&self) -> Vec<&'static Spec> {
        vec![&SERVER]
    }

    fn tokens(&self) -> &'static [(&'static str, &'static str)] {
        // `sorry`, `admit`.
        &[("leanSorryLike", "constant.builtin")]
    }

    fn panel(&self, language: &Language) -> Option<Box<dyn Panel>> {
        std::ptr::eq(language, &LEAN).then(|| Box::new(Goals::default()) as Box<dyn Panel>)
    }
}

/// What Lean says about the cursor, as its infoview shows it: the proof state
/// there (`$/lean/plainGoal`), or else the type expected (`$/lean/plainTermGoal`);
/// and how far it's got checking the file (`$/lean/fileProgress`).
#[derive(Default)]
struct Goals {
    /// The goals at the cursor, when it's in a proof (none left: it's done).
    goals: Option<Vec<String>>,
    /// The type expected at the cursor, when it's in a term.
    expected: Option<String>,
    /// The first line Lean is still checking.
    checking: Option<u32>,
}

impl Panel for Goals {
    fn title(&self) -> &'static str {
        "Lean"
    }

    fn requests(&self) -> &'static [&'static str] {
        &["$/lean/plainGoal", "$/lean/plainTermGoal"]
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

    fn working_on(&self) -> Option<u32> {
        self.checking
    }

    fn blocks(&self) -> Vec<Block> {
        let code = |text: &String| Block::Code { text: text.clone(), stands_out: Some('⊢') };
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn it_shows_the_goals_or_the_type_expected() {
        let mut goals = Goals::default();
        assert!(matches!(goals.blocks().as_slice(), [Block::Muted(_)]));
        goals.answered(vec![Some(json!({ "goals": ["n : Nat\n⊢ n + 0 = n"] })), None]);
        assert_eq!(goals.blocks(), [Block::Heading("1 goal".into()), Block::Code { text: "n : Nat\n⊢ n + 0 = n".into(), stands_out: Some('⊢') }]);
        goals.answered(vec![Some(json!({ "goals": [] })), None]);
        assert_eq!(goals.blocks(), [Block::Good("No goals".into())]);
        goals.answered(vec![None, Some(json!({ "goal": "Nat" }))]);
        assert_eq!(goals.blocks()[0], Block::Heading("Expected type".into()));
    }

    #[test]
    fn once_its_done_checking_the_goals_are_asked_for_again() {
        let mut goals = Goals::default();
        let progress = |line: Option<u32>| json!({ "processing": line.map_or(vec![], |l| vec![json!({ "range": { "start": { "line": l } } })]) });
        assert!(!goals.notification("$/lean/fileProgress", &progress(Some(3))), "still going");
        assert_eq!(goals.working_on(), Some(3));
        assert!(goals.notification("$/lean/fileProgress", &progress(None)), "done");
        assert_eq!(goals.working_on(), None);
        assert!(!goals.notification("$/lean/fileProgress", &progress(None)), "and only once");
        assert!(!goals.notification("something else", &json!({})));
    }

    #[test]
    fn it_knows_its_files_and_symbols() {
        let abbreviations = LEAN.abbreviations.unwrap();
        assert_eq!(abbreviations.eager("alpha"), Some("α"));
        assert_eq!(abbreviations.finished("N"), Some("ℕ"));
        assert_eq!(LEAN.highlight("def x := 1\n").first().map(|s| s.range.clone()), Some(0..3), "def");
    }
}
