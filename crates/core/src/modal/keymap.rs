//! The keymap: which keys run which [`Command`] in normal and select mode
//! (and AI mode, looking around), as helix keeps it: a trie, where `g`,
//! `space`, `z`, `m`, `]`, `[` and `ctrl-w` lead on to more keys.
//!
//! [`DEFAULT`] is the keymap as data, in the cheatsheet's sections and
//! lines, so [`Keymap::cheatsheet`] comes from what's bound. Your own keys
//! (the config's `"keys"`) go over it: a command by name, a `:` command, or
//! `no_op` to unbind a key.

use super::Key;
use super::command::Command::{self, *};

/// One line of the cheatsheet: its keys and the commands they run, and what they do.
type Line = (&'static [(&'static str, Command)], &'static str);

/// The keys of normal and select mode, as helix has them (Ghostwrite's own
/// are under `space`), by the cheatsheet's sections and lines. Keys
/// pressed one after another are written apart: `g g`, `space f`.
pub const DEFAULT: &[(&str, &[Line])] = &[
    (
        "Move",
        &[
            (&[("h", MoveCharLeft), ("j", MoveLineDown), ("k", MoveLineUp), ("l", MoveCharRight)], "left down up right"),
            (&[("left", MoveCharLeft), ("down", MoveLineDown), ("up", MoveLineUp), ("right", MoveCharRight)], "the same"),
            (&[("w", MoveNextWordStart), ("b", MovePrevWordStart), ("e", MoveNextWordEnd)], "next word, back, word end"),
            (&[("W", MoveNextLongWordStart), ("B", MovePrevLongWordStart), ("E", MoveNextLongWordEnd)], "the same by WORD (between spaces)"),
            (&[("f", FindNextChar), ("t", FindTillChar), ("F", FindPrevChar), ("T", TillPrevChar)], "find / till a char, over lines (⏎: line end; F T: back)"),
            (&[("alt-.", RepeatLastMotion)], "find again (and mi ma ]p [p)"),
            (&[("g g", GotoFileStart), ("g e", GotoLastLine), ("G", GotoLine)], "file start, last line; 10G: line 10"),
            (&[("g h", GotoLineStart), ("g l", GotoLineEnd), ("g s", GotoFirstNonwhitespace)], "line start, end, first non-blank"),
            (&[("home", GotoLineStart), ("end", GotoLineEnd)], "line start, end"),
            (&[("g t", GotoWindowTop), ("g c", GotoWindowCenter), ("g b", GotoWindowBottom)], "top, middle, bottom line on screen"),
            (
                &[("z z", AlignViewCenter), ("z t", AlignViewTop), ("z b", AlignViewBottom), ("z c", AlignViewCenter)],
                "scroll: this line to the middle, top, bottom (zc: middle)",
            ),
            (&[("] d", GotoNextDiag), ("[ d", GotoPrevDiag), ("] p", GotoNextParagraph), ("[ p", GotoPrevParagraph)], "next, previous diagnostic; paragraph"),
            (&[("g .", GotoLastModification)], "where the last change was"),
            (&[("ctrl-d", PageCursorHalfDown), ("ctrl-u", PageCursorHalfUp)], "half page down, up"),
            (&[("ctrl-f", PageCursorDown), ("ctrl-b", PageCursorUp), ("pagedown", PageCursorDown), ("pageup", PageCursorUp)], "page down, up"),
            (&[("g n", GotoNextBuffer), ("g p", GotoPreviousBuffer)], "next, previous tab (4gn: four along)"),
            (&[("g N", MoveBufferNext), ("g P", MoveBufferPrevious)], "move the tab along (4gN: four places)"),
            (&[("g d", GotoDefinition), ("g r", GotoReference)], "go to definition; through the references"),
            (&[("g f", GotoFile)], "the file named here (path:line:col)"),
            (&[("ctrl-o", JumpBackward), ("ctrl-i", JumpForward)], "back where a jump came from, forward"),
        ],
    ),
    (
        "Select",
        &[
            (&[("v", SelectMode)], "select mode (motions extend)"),
            (&[("esc", NormalMode)], "normal mode; with cursors, back to one"),
            (&[("C", CopySelectionOnNextLine), ("alt-C", CopySelectionOnPrevLine)], "copy the selection to the line below, above"),
            (&[("s", SelectRegex), ("alt-s", SplitSelectionOnNewline)], "a cursor on each match in the selection; each line"),
            (&[(",", KeepPrimarySelection), ("alt-,", RemovePrimarySelection)], "back to one cursor; drop the main one"),
            (&[("(", RotateSelectionsBackward), (")", RotateSelectionsForward)], "the main cursor: the one before, after"),
            (
                &[("alt-minus", MergeSelections), ("alt-_", MergeConsecutiveSelections), ("&", AlignSelections)],
                "merge selections (those touching); line them up",
            ),
            (
                &[("x", ExtendLineBelow), ("X", ExtendToLineBounds), ("alt-x", ShrinkToLineBounds), ("%", SelectAll)],
                "select line (again: next line), out to, in to whole lines; all",
            ),
            (&[(";", CollapseSelection), ("alt-;", FlipSelections), ("_", TrimSelections)], "collapse, flip, trim selection"),
        ],
    ),
    (
        "Edit",
        &[
            (&[("i", InsertMode), ("a", AppendMode), ("I", InsertAtLineStart), ("A", InsertAtLineEnd)], "insert before, after, line start, end"),
            (&[("o", OpenBelow), ("O", OpenAbove)], "open line below, above (3o: three, a cursor each)"),
            (
                &[("d", DeleteSelection), ("c", ChangeSelection), ("alt-d", DeleteSelectionNoyank), ("alt-c", ChangeSelectionNoyank)],
                "delete, change selection (alt: no yank)",
            ),
            (&[("y", Yank), ("p", PasteAfter), ("P", PasteBefore), ("R", ReplaceWithYanked)], "yank, paste after, before; replace with yanked"),
            (&[("\"", SelectRegister)], "use register r for the next y d c p (+: clipboard)"),
            (&[("r", Replace)], "replace with char"),
            (&[("~", SwitchCase), ("`", SwitchToLowercase), ("alt-`", SwitchToUppercase)], "switch case, lower, upper"),
            (&[(">", Indent), ("<", Unindent), ("J", JoinSelections), ("alt-J", JoinSelectionsSpace)], "indent, unindent; join lines (alt: select the spaces)"),
            (&[("] space", AddNewlineBelow), ("[ space", AddNewlineAbove)], "blank line below, above"),
            (&[("ctrl-a", Increment), ("ctrl-x", Decrement), ("ctrl-c", ToggleComments)], "number up, down; comment out / in"),
            (&[(".", RepeatLastChange), ("Q", RecordMacro), ("q", ReplayMacro)], "repeat the last change; record, play a macro"),
            (&[("u", Undo), ("U", Redo)], "undo, redo"),
        ],
    ),
    (
        "Match  m",
        &[
            (&[("m m", MatchBrackets)], "matching bracket"),
            (&[("m s", SurroundAdd)], "surround with a pair"),
            (&[("m r", SurroundReplace)], "replace surrounding pair"),
            (&[("m d", SurroundDelete)], "delete surrounding pair"),
            (&[("m i", SelectTextobjectInner), ("m a", SelectTextobjectAround)], "select inside / around: w W p ( [ { < ' \" ` m, and f t a c T e x"),
            (
                &[
                    ("] f", GotoNextFunction),
                    ("[ f", GotoPrevFunction),
                    ("] t", GotoNextClass),
                    ("[ t", GotoPrevClass),
                    ("] a", GotoNextParameter),
                    ("[ a", GotoPrevParameter),
                    ("] c", GotoNextComment),
                    ("[ c", GotoPrevComment),
                    ("] T", GotoNextTest),
                    ("[ T", GotoPrevTest),
                    ("] e", GotoNextEntry),
                    ("[ e", GotoPrevEntry),
                    ("] x", GotoNextXmlElement),
                    ("[ x", GotoPrevXmlElement),
                ],
                "next, previous function (t type, a argument, c comment, T test, e entry, x element)",
            ),
            (&[("alt-o", ExpandSelection), ("alt-i", ShrinkSelection)], "out to the syntax node around, back in"),
            (
                &[("alt-n", SelectNextSibling), ("alt-p", SelectPrevSibling), ("alt-a", SelectAllSiblings), ("alt-I", SelectAllChildren)],
                "next, previous node; a cursor on each beside, inside",
            ),
            (&[("alt-e", MoveParentNodeEnd), ("alt-b", MoveParentNodeStart)], "to the end, start of the node"),
        ],
    ),
    (
        "Search",
        &[
            (&[("/", Search), ("?", Rsearch)], "search the file, forward, back (esc goes back)"),
            (&[("n", SearchNext), ("N", SearchPrev)], "next match, previous (select mode: add it)"),
            (&[("*", SearchSelectionDetectWordBounds), ("alt-*", SearchSelection)], "search for the selection (as a word; anywhere)"),
        ],
    ),
    (
        "Space",
        &[
            (&[("space a", Ask)], "ask about the line or selection"),
            (&[("space e", FileExplorer)], "file tree (⌘B too)"),
            (&[("space t", Terminal)], "terminal: type in it; esc esc reads it (⌘J too)"),
            (&[("space f", FilePicker), ("space b", BufferPicker)], "pick a file, an open file (fuzzy)"),
            (&[("space /", GlobalSearch)], "search the project"),
            (&[("space k", Hover), ("space d", FileDiagnostics)], "docs here; this file's diagnostics"),
            (&[("space r", RenameSymbol), ("space .", CodeAction)], "rename; the server's code actions here"),
            (&[("space s", SymbolPicker)], "pick one of the file's symbols"),
            (&[("space j", GotoNextProblem)], "next error (or warning); a file's pages follow"),
            (&[("space y", YankToClipboard), ("space p", PasteClipboardAfter), ("space P", PasteClipboardBefore)], "clipboard yank, paste after, before"),
            (&[("space ?", Cheatsheet)], "this, every key"),
            (&[("ctrl-w v", Vsplit)], "two panes: this file's pages, or the next tab"),
            (&[("ctrl-w w", RotateView), ("ctrl-w h", RotateView), ("ctrl-w l", RotateView)], "the other pane"),
            (&[("ctrl-w q", Wonly), ("ctrl-w o", Wonly)], "only this one"),
        ],
    ),
    ("Command line  :", &[(&[(":", CommandMode)], "the command line")]),
];

/// The cheatsheet's section for keys of yours it has no line for.
pub const YOURS: &str = "Your keys";

/// What a key sequence is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    Command(Command),
    /// A `:` command (without its `:`), of yours: `"ctrl-s": ":w"`.
    Typed(String),
}

/// Where a binding shows in the cheatsheet: on a line of [`DEFAULT`] (its
/// section and line), or with the rest of yours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shown {
    At(usize, usize),
    Yours,
}

#[derive(Debug, Clone)]
struct Binding {
    bound: Bound,
    shown: Shown,
    /// Bound before those with a higher number: the order keys are shown in.
    order: usize,
}

#[derive(Debug, Clone)]
enum Node {
    Bound(Binding),
    /// Keys that lead on to more.
    Keys(Vec<(Key, Node)>),
}

/// The keys of normal and select mode.
#[derive(Debug, Clone)]
pub struct Keymap {
    root: Vec<(Key, Node)>,
    /// How many bindings have been made, for their order.
    made: usize,
}

/// What a key sequence comes to.
pub(super) enum Found<'a> {
    Bound(&'a Bound),
    /// More keys to come.
    Keys,
    Nothing,
}

/// A section of the cheatsheet.
#[derive(Debug, Clone)]
pub struct Section {
    pub title: &'static str,
    pub lines: Vec<CheatLine>,
}

/// A line of the cheatsheet: keys, what they wait for, and what they do.
#[derive(Debug, Clone)]
pub struct CheatLine {
    pub keys: Vec<Vec<Key>>,
    pub arg: Option<&'static str>,
    pub what: String,
}

impl CheatLine {
    /// The keys as the cheatsheet writes them: `h j k l`, `space f  space b`, `f t ‹c›`.
    pub fn shown(&self) -> String {
        let keys: Vec<String> = self.keys.iter().map(|keys| sequence(keys)).collect();
        let gap = if keys.iter().any(|keys| keys.contains(' ')) { "  " } else { " " };
        let shown = keys.join(gap);
        match self.arg {
            Some(arg) => format!("{shown} {arg}"),
            None => shown,
        }
    }
}

impl Default for Keymap {
    /// [`DEFAULT`].
    fn default() -> Self {
        let mut keymap = Keymap { root: vec![], made: 0 };
        for (s, (_, lines)) in DEFAULT.iter().enumerate() {
            for (l, (bindings, _)) in lines.iter().enumerate() {
                for (keys, command) in bindings.iter() {
                    let keys = parse_keys(keys).unwrap_or_else(|| panic!("the default keymap's {keys:?}"));
                    keymap.bind(&keys, Bound::Command(*command), Shown::At(s, l));
                }
            }
        }
        keymap
    }
}

impl Keymap {
    /// [`DEFAULT`], made once.
    pub fn standard() -> &'static Keymap {
        static STANDARD: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
        STANDARD.get_or_init(Keymap::default)
    }

    /// This keymap with your keys over it (the config's `"keys"`), and
    /// what's wrong with those that couldn't be bound.
    pub fn with_yours(mut self, yours: &[(String, String)]) -> (Keymap, Vec<String>) {
        let wrong = yours.iter().filter_map(|(keys, to)| self.bind_yours(keys, to).err()).collect();
        (self, wrong)
    }

    /// `keys` (`"space w"`) bound to `to`: a `:` command (`":wa"`), a
    /// command's name (`"extend_line_below"`), or `no_op`, which unbinds them.
    pub fn bind_yours(&mut self, keys: &str, to: &str) -> Result<(), String> {
        let Some(sequence) = parse_keys(keys) else { return Err(format!("\"{keys}\" isn't keys (as in \"space w\", \"ctrl-s\")")) };
        if let Some(line) = to.strip_prefix(':') {
            self.bind(&sequence, Bound::Typed(line.trim().to_string()), Shown::Yours);
            return Ok(());
        }
        match Command::named(to.trim()) {
            Some(NoOp) => self.unbind(&sequence),
            // Where the command is in the cheatsheet already, it shows there.
            Some(command) => self.bind(&sequence, Bound::Command(command), shown_at(command)),
            None => return Err(format!("\"{keys}\": no command \"{to}\" (a : command starts with :)")),
        }
        Ok(())
    }

    /// `keys` bound, over whatever was: a binding on the way to them gives way.
    fn bind(&mut self, keys: &[Key], bound: Bound, shown: Shown) {
        let Some((last, before)) = keys.split_last() else { return };
        let binding = Binding { bound, shown, order: self.made };
        self.made += 1;
        let mut nodes = &mut self.root;
        for key in before {
            let at = match nodes.iter().position(|(k, _)| k == key) {
                Some(at) => at,
                None => {
                    nodes.push((*key, Node::Keys(vec![])));
                    nodes.len() - 1
                }
            };
            let node = &mut nodes[at].1;
            if let Node::Bound(_) = node {
                *node = Node::Keys(vec![]);
            }
            let Node::Keys(next) = node else { unreachable!("made keys just above") };
            nodes = next;
        }
        match nodes.iter_mut().find(|(k, _)| k == last) {
            Some((_, node)) => *node = Node::Bound(binding),
            None => nodes.push((*last, Node::Bound(binding))),
        }
    }

    /// `keys` unbound, and whatever they led on to.
    fn unbind(&mut self, keys: &[Key]) {
        fn from(nodes: &mut Vec<(Key, Node)>, keys: &[Key]) {
            let Some((first, rest)) = keys.split_first() else { return };
            let Some(at) = nodes.iter().position(|(k, _)| k == first) else { return };
            if rest.is_empty() {
                nodes.remove(at);
            } else if let Node::Keys(next) = &mut nodes[at].1 {
                from(next, rest);
                // Keys that lead nowhere now go too.
                if next.is_empty() {
                    nodes.remove(at);
                }
            }
        }
        from(&mut self.root, keys);
    }

    /// What `keys` come to. After a ctrl key (`ctrl-w`), the next keys can be
    /// had with ctrl held too: `ctrl-w ctrl-v` is `ctrl-w v`, as in helix.
    pub(super) fn find(&self, keys: &[Key]) -> Found<'_> {
        let held = matches!(keys.first(), Some(Key::Ctrl(_)));
        let mut nodes = &self.root;
        for (i, &key) in keys.iter().enumerate() {
            let get = |key: Key| nodes.iter().find(|(k, _)| *k == key).map(|(_, node)| node);
            let node = match (get(key), key) {
                (Some(node), _) => node,
                (None, Key::Ctrl(c)) if held && i > 0 => match get(Key::Char(c)) {
                    Some(node) => node,
                    None => return Found::Nothing,
                },
                _ => return Found::Nothing,
            };
            match node {
                Node::Bound(binding) if i + 1 == keys.len() => return Found::Bound(&binding.bound),
                Node::Bound(_) => return Found::Nothing,
                Node::Keys(next) => nodes = next,
            }
        }
        if keys.is_empty() { Found::Nothing } else { Found::Keys }
    }

    /// Every binding: its keys and what they're bound to.
    pub fn bindings(&self) -> Vec<(Vec<Key>, &Bound)> {
        self.all().into_iter().map(|(keys, binding)| (keys, &binding.bound)).collect()
    }

    fn all(&self) -> Vec<(Vec<Key>, &Binding)> {
        fn walk<'a>(nodes: &'a [(Key, Node)], keys: &mut Vec<Key>, all: &mut Vec<(Vec<Key>, &'a Binding)>) {
            for (key, node) in nodes {
                keys.push(*key);
                match node {
                    Node::Bound(binding) => all.push((keys.clone(), binding)),
                    Node::Keys(next) => walk(next, keys, all),
                }
                keys.pop();
            }
        }
        let mut all = vec![];
        walk(&self.root, &mut vec![], &mut all);
        all
    }

    /// The `space ?` cheatsheet: every binding, on the lines [`DEFAULT`]
    /// puts it on (keys of yours that have none at the end, under [`YOURS`]).
    pub fn cheatsheet(&self) -> Vec<Section> {
        let mut all = self.all();
        all.sort_by_key(|(_, binding)| (binding.shown, binding.order));
        let mut sections: Vec<Section> = vec![];
        let mut last = None;
        for (keys, binding) in all {
            let arg = match &binding.bound {
                Bound::Command(command) => command.arg(),
                Bound::Typed(_) => None,
            };
            let (title, what) = match (binding.shown, &binding.bound) {
                (Shown::At(s, l), _) => (DEFAULT[s].0, DEFAULT[s].1[l].1.to_string()),
                (Shown::Yours, Bound::Typed(line)) => (YOURS, format!(":{line}")),
                (Shown::Yours, Bound::Command(command)) => (YOURS, command.doc().to_string()),
            };
            if sections.last().is_none_or(|section| section.title != title) {
                sections.push(Section { title, lines: vec![] });
            }
            let lines = &mut sections.last_mut().expect("pushed just above").lines;
            match lines.last_mut() {
                Some(line) if last == Some(binding.shown) && binding.shown != Shown::Yours => {
                    line.keys.push(keys);
                    line.arg = line.arg.or(arg);
                }
                _ => lines.push(CheatLine { keys: vec![keys], arg, what }),
            }
            last = Some(binding.shown);
        }
        sections
    }
}

/// The line of [`DEFAULT`] `command` is first on, if it's on one.
fn shown_at(command: Command) -> Shown {
    for (s, (_, lines)) in DEFAULT.iter().enumerate() {
        for (l, (bindings, _)) in lines.iter().enumerate() {
            if bindings.iter().any(|(_, c)| *c == command) {
                return Shown::At(s, l);
            }
        }
    }
    Shown::Yours
}

impl Key {
    /// The key called `name`: `x`, `space`, `ctrl-w`, `alt-minus`, `esc`, `enter`, `left`.
    pub fn named(name: &str) -> Option<Key> {
        let char_of = |name: &str| match name {
            "space" => Some(' '),
            "minus" => Some('-'),
            _ => {
                let mut chars = name.chars();
                chars.next().filter(|_| chars.next().is_none())
            }
        };
        if let Some(rest) = name.strip_prefix("ctrl-").filter(|rest| !rest.is_empty()) {
            return char_of(rest).map(Key::Ctrl);
        }
        if let Some(rest) = name.strip_prefix("alt-").filter(|rest| !rest.is_empty()) {
            return char_of(rest).map(Key::Alt);
        }
        Some(match name {
            "esc" | "escape" => Key::Escape,
            "enter" | "ret" | "return" => Key::Enter,
            "tab" => Key::Tab,
            "shift-tab" | "backtab" => Key::BackTab,
            "backspace" | "bs" => Key::Backspace,
            "del" | "delete" => Key::Delete,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            name => Key::Char(char_of(name)?),
        })
    }

    /// Its name, as [`Key::named`] takes it.
    pub fn name(self) -> String {
        let char_name = |c: char| match c {
            ' ' => "space".to_string(),
            c => c.to_string(),
        };
        match self {
            Key::Char(c) => char_name(c),
            Key::Ctrl(c) => format!("ctrl-{}", char_name(c)),
            Key::Alt('-') => "alt-minus".to_string(),
            Key::Alt(c) => format!("alt-{}", char_name(c)),
            Key::Escape => "esc".into(),
            Key::Enter => "enter".into(),
            Key::Tab => "tab".into(),
            Key::BackTab => "shift-tab".into(),
            Key::Backspace => "backspace".into(),
            Key::Delete => "del".into(),
            Key::Left => "left".into(),
            Key::Right => "right".into(),
            Key::Up => "up".into(),
            Key::Down => "down".into(),
            Key::Home => "home".into(),
            Key::End => "end".into(),
            Key::PageUp => "pageup".into(),
            Key::PageDown => "pagedown".into(),
        }
    }
}

/// Keys written apart (`space w`, `g g`), if each is a key.
pub fn parse_keys(keys: &str) -> Option<Vec<Key>> {
    let keys: Option<Vec<Key>> = keys.split_whitespace().map(Key::named).collect();
    keys.filter(|keys| !keys.is_empty())
}

/// Keys one after another, as helix writes them: run together if each is
/// a char (`gg`, `]d`), else apart (`space f`, `ctrl-w v`).
pub fn sequence(keys: &[Key]) -> String {
    let names: Vec<String> = keys.iter().map(|key| key.name()).collect();
    if names.iter().all(|name| name.chars().count() == 1) { names.concat() } else { names.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(keys: &str) -> Vec<Key> {
        parse_keys(keys).unwrap()
    }

    fn command(keymap: &Keymap, keys: &str) -> Option<Command> {
        match keymap.find(&self::keys(keys)) {
            Found::Bound(Bound::Command(command)) => Some(*command),
            _ => None,
        }
    }

    #[test]
    fn keys_are_named_and_read_back() {
        for name in ["x", "space", "ctrl-w", "alt-minus", "alt-.", "esc", "enter", "left", "pagedown", "shift-tab"] {
            assert_eq!(Key::named(name).map(Key::name).as_deref(), Some(name));
        }
        assert_eq!(Key::named("escape"), Some(Key::Escape));
        assert_eq!(Key::named("ctrl-"), None);
        assert_eq!(Key::named("wat"), None);
        assert_eq!(sequence(&keys("g g")), "gg");
        assert_eq!(sequence(&keys("space f")), "space f");
    }

    #[test]
    fn the_default_keymap_binds_each_key_once() {
        let mut seen = std::collections::HashSet::new();
        for (_, lines) in DEFAULT {
            for (bindings, _) in lines.iter() {
                for (keys, _) in bindings.iter() {
                    assert!(seen.insert(self::keys(keys)), "{keys} twice");
                }
            }
        }
        assert_eq!(Keymap::standard().bindings().len(), seen.len());
    }

    #[test]
    fn prefixes_lead_on_and_ctrl_can_be_held() {
        let keymap = Keymap::standard();
        assert!(matches!(keymap.find(&keys("g")), Found::Keys));
        assert_eq!(command(keymap, "g g"), Some(GotoFileStart));
        assert_eq!(command(keymap, "ctrl-w ctrl-v"), Some(Vsplit), "as ctrl-w v");
        assert_eq!(command(keymap, "g ctrl-g"), None, "only after a ctrl key");
        assert!(matches!(keymap.find(&keys("g z")), Found::Nothing));
    }

    #[test]
    fn your_keys_go_over_the_default() {
        let yours = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        let (keymap, wrong) = Keymap::default().with_yours(&yours(&[
            ("Z", "extend_line_below"),
            ("x", "no_op"),
            ("ctrl-s", ":w"),
            ("space w", ":wa"),
            ("g", "goto_file_start"),
            ("Z Z", ":x"),
            ("q", "extend_lin_below"),
            ("wat wat", "undo"),
        ]));
        assert_eq!(command(&keymap, "Z Z"), None, "Z became a prefix");
        assert!(matches!(keymap.find(&keys("Z Z")), Found::Bound(Bound::Typed(line)) if line == "x"));
        assert_eq!(command(&keymap, "x"), None, "unbound");
        assert!(matches!(keymap.find(&keys("ctrl-s")), Found::Bound(Bound::Typed(line)) if line == "w"));
        assert!(matches!(keymap.find(&keys("space w")), Found::Bound(Bound::Typed(line)) if line == "wa"));
        assert_eq!(command(&keymap, "space f"), Some(FilePicker), "the rest of space is still there");
        assert_eq!(command(&keymap, "g"), Some(GotoFileStart), "and g isn't a prefix any more");
        assert_eq!(command(&keymap, "q"), Some(ReplayMacro), "a wrong name leaves the key as it was");
        assert_eq!(wrong.len(), 2);
        assert!(wrong[0].contains("no command \"extend_lin_below\""), "{}", wrong[0]);
        assert!(wrong[1].contains("isn't keys"), "{}", wrong[1]);
    }

    #[test]
    fn the_cheatsheet_has_your_keys() {
        let mut keymap = Keymap::default();
        keymap.bind_yours("Z", "extend_line_below").unwrap();
        keymap.bind_yours("x", "no_op").unwrap();
        keymap.bind_yours("ctrl-s", ":w").unwrap();
        let sheet = keymap.cheatsheet();
        let line = |what: &str| sheet.iter().flat_map(|s| &s.lines).find(|line| line.what.starts_with(what)).map(CheatLine::shown);
        assert_eq!(line("select line").as_deref(), Some("X alt-x % Z"), "Z joins x's line, and x is gone");
        assert_eq!(sheet.last().map(|s| s.title), Some(YOURS));
        assert_eq!(line(":w").as_deref(), Some("ctrl-s"));
        assert_eq!(line("find / till").as_deref(), Some("f t F T ‹c›"));
        assert_eq!(line("pick a file").as_deref(), Some("space f  space b"));
    }
}
