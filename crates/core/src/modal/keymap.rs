//! The keymap: which keys run which [`Command`] in normal and select mode
//! (and AI mode, looking around), as helix keeps it: a trie, where `g`,
//! `space`, `space w`, `z`, `m`, `]`, `[` lead on to more keys. `space` and a
//! letter is the one way from view to view (`space e`, `t`, `x`, and the keys
//! plugins' views declare), and the panes are under `space w`, as helix's window
//! mode is.
//!
//! [`DEFAULT`] is the keymap as data, in the cheatsheet's sections and
//! lines, so [`Keymap::cheatsheet`] comes from what's bound. Each view that
//! isn't the code has a layer of its own over it, as Zed's key contexts are:
//! [`VIEWS`] for the editor's, and for a plugin's, the layer of what it shows
//! ([`SHOWS`]). A key is looked up in the layer of the view with the keys first,
//! then in the code's. The keys plugins' views declare go in the code's where
//! they're free ([`Keymap::with_views`]): the editor's come first, then the plugin
//! installed first. Your own keys (the config's `"keys"`, for the code and
//! every view, or `"files"`, `"goals"` and the like for one) go over them all:
//! a command by name, `focus_view` and a view's name, a `:` command, or `no_op`
//! to unbind a key.

use super::command::Command::{self, *};
use super::{Key, PluginView, Shows, ViewKind};

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
            (&[("space f", FilePicker), ("space b", BufferPicker)], "pick a file, an open file (fuzzy)"),
            (&[("space /", GlobalSearch)], "search the project"),
            (&[("space k", Hover), ("space d", FileDiagnostics)], "docs here; this file's diagnostics"),
            (&[("space r", RenameSymbol), ("space .", CodeAction)], "rename; the server's code actions here"),
            (&[("space s", SymbolPicker)], "pick one of the file's symbols"),
            (&[("space j", GotoNextProblem)], "next error (or warning); a file's pages follow"),
            (&[("space y", YankToClipboard), ("space p", PasteClipboardAfter), ("space P", PasteClipboardBefore)], "clipboard yank, paste after, before"),
            (&[("space ?", Cheatsheet)], "this, every key"),
        ],
    ),
    (
        "Views and panes",
        &[
            (&[("space e", FileExplorer)], "Files, the file tree (⌘B too); again: back to the code"),
            (&[("space t", Terminal)], "the terminal, typing in it (⌘J too); again: back to the code"),
            (&[("space x", Debugger)], "the debugger, while something's being debugged; again: back to the code"),
            (&[("space w v", Vsplit)], "two panes: this file's pages, or the next tab"),
            (&[("space w w", RotateView)], "the other pane"),
            (&[("space w q", Wonly), ("space w o", Wonly)], "only this one"),
        ],
    ),
    ("Command line  :", &[(&[(":", CommandMode)], "the command line")]),
];

/// The keys of each of the editor's views that isn't the code, over the code's, by the
/// cheatsheet's sections and lines. Elsewhere the code's keys work as ever: on the
/// terminal they move through the text, and in a view with no text only those that
/// work anywhere do (see [`Command::anywhere`]).
pub const VIEWS: &[(ViewKind, &str, &[Line])] = &[
    (
        ViewKind::Files,
        "Files  space e",
        &[
            (&[("j", FilesDown), ("down", FilesDown), ("k", FilesUp), ("up", FilesUp)], "down, up a row (3j: three)"),
            (&[("g g", FilesFirst), ("home", FilesFirst), ("g e", FilesLast), ("G", FilesLast), ("end", FilesLast)], "first, last row"),
            (&[("l", ExpandFolder), ("right", ExpandFolder), ("h", CollapseFolder), ("left", CollapseFolder)], "open a folder, preview a file; close, up"),
            (&[("enter", OpenFile)], "open the file, in the code (a folder: open, close)"),
            (&[("a", AddFile), ("r", RenameFile), ("d", DeleteFile)], "add (name/: a folder), rename, trash"),
            (&[("esc", FocusCode)], "back to the code (space e too)"),
        ],
    ),
    (
        ViewKind::Terminal,
        "Terminal  space t",
        &[
            (&[("enter", GotoFile)], "open the file a line names, at its line (gf too)"),
            (&[("esc", FocusCode)], "back to the code (from select mode, normal mode)"),
        ],
    ),
    (
        ViewKind::Debugger,
        "Debugger  space x",
        &[
            (&[("j", ScrollDown), ("down", ScrollDown), ("k", ScrollUp), ("up", ScrollUp)], "scroll down, up"),
            (&[("ctrl-d", ScrollPageDown), ("ctrl-u", ScrollPageUp)], "a screen down, up"),
            (&[("esc", FocusCode)], "back to the code (space x too)"),
        ],
    ),
];

/// The keys of a plugin's view, by what it shows, over the code's: each such view's
/// section of the cheatsheet has them, under its title. Its plugin's keys (`"in"`) and
/// yours for it (by its name) go over them.
pub const SHOWS: &[(Shows, &[Line])] = &[
    (
        Shows::Pages,
        &[
            (&[("j", ScrollDown), ("down", ScrollDown), ("k", ScrollUp), ("up", ScrollUp)], "scroll down, up"),
            (&[("ctrl-d", ScrollPageDown), ("ctrl-u", ScrollPageUp)], "a screen down, up"),
            (&[("+", ZoomIn), ("-", ZoomOut), ("=", ZoomReset)], "zoom in, out; fit the view (⌘= ⌘- ⌘0)"),
        ],
    ),
    (
        Shows::Blocks,
        &[
            (&[("j", ScrollDown), ("down", ScrollDown), ("k", ScrollUp), ("up", ScrollUp)], "scroll down, up"),
            (&[("ctrl-d", ScrollPageDown), ("ctrl-u", ScrollPageUp)], "a screen down, up"),
            (&[("esc", FocusCode)], "back to the code"),
        ],
    ),
];

/// Where a key is looked up: the layer of one of the editor's views, the layer of what
/// a plugin's view shows, or yours for the plugin's views of a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Layer {
    Of(ViewKind),
    Shows(Shows),
    Named(&'static str),
}

/// Every static section of the cheatsheet the keymap has, and the layer its keys are in:
/// [`DEFAULT`]'s, [`VIEWS`]', then [`SHOWS`]'.
fn sections() -> impl Iterator<Item = (Layer, &'static str, &'static [Line])> {
    let default = DEFAULT.iter().map(|(title, lines)| (Layer::Of(ViewKind::Code), *title, *lines));
    let views = VIEWS.iter().map(|(view, title, lines)| (Layer::Of(*view), *title, *lines));
    default.chain(views).chain(SHOWS.iter().map(|(shows, lines)| (Layer::Shows(*shows), shows.name(), *lines)))
}

/// The layers `view`'s keys are looked up in, the first first.
fn layers_of(view: ViewKind) -> Vec<Layer> {
    match view {
        ViewKind::Code => vec![Layer::Of(ViewKind::Code)],
        ViewKind::Plugin(id) => vec![Layer::Named(id.name), Layer::Shows(id.shows), Layer::Of(ViewKind::Code)],
        view => vec![Layer::Of(view), Layer::Of(ViewKind::Code)],
    }
}

/// The layer your keys for `view` go in.
fn yours_for(view: ViewKind) -> Layer {
    match view {
        ViewKind::Plugin(id) => Layer::Named(id.name),
        view => Layer::Of(view),
    }
}

/// A plugin's view that has no key, as the one it declares is taken: by the editor
/// (the editor's keys come first), or by a view of a plugin installed before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    pub view: PluginView,
    pub keys: &'static str,
    pub with: Taken,
}

/// Who has the keys a plugin's view declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    /// The editor, for this (a command's name, or keys they lead on to).
    Editor(String),
    View(PluginView),
}

impl Clash {
    /// What it comes to, for the status line and `:plugin list`.
    pub fn said(&self) -> String {
        let PluginView { id, .. } = self.view;
        let whose = match &self.with {
            Taken::Editor(what) => format!("the editor's ({what})"),
            Taken::View(other) => format!("{}'s {} view's", other.id.plugin, other.id.name),
        };
        format!("{}'s {} view has no key: {} is {whose}; give it one of yours, as \"space …\": \"focus_view {}\"", id.plugin, id.name, self.keys, id.name)
    }
}

/// The cheatsheet's section for keys of yours it has no line for.
pub const YOURS: &str = "Your keys";
/// The section the keys of plugins' views are in, with the editor's.
const VIEWS_AND_PANES: &str = "Views and panes";

/// What a key sequence is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    Command(Command),
    /// A plugin's view, by its name: the key it declares, or yours (`"focus_view goals"`).
    View(&'static str),
    /// A `:` command (without its `:`), of yours: `"ctrl-s": ":w"`.
    Typed(String),
}

/// Where a binding shows in the cheatsheet: on a line of [`sections`] (its
/// section and line), on the line of a plugin's view (by its first in the keymap's
/// views), or with the rest of yours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shown {
    At(usize, usize),
    View(usize),
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

type Trie = Vec<(Key, Node)>;

/// The keys of normal and select mode: the code's, and each view's layer over them.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// Each layer that has keys.
    layers: Vec<(Layer, Trie)>,
    /// How many bindings have been made, for their order.
    made: usize,
    /// The plugins' views, the plugin installed first first.
    views: Vec<PluginView>,
    /// Those of them whose keys were taken.
    clashes: Vec<Clash>,
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
    pub title: String,
    pub lines: Vec<CheatLine>,
}

/// A line of the cheatsheet: the view its keys are for, the keys, what they wait for, and what they do.
#[derive(Debug, Clone)]
pub struct CheatLine {
    pub view: ViewKind,
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
    /// [`DEFAULT`], [`VIEWS`] and [`SHOWS`]: the editor's keys, with no plugin's views.
    fn default() -> Self {
        let mut keymap = Keymap { layers: vec![], made: 0, views: vec![], clashes: vec![] };
        for (s, (layer, _, lines)) in sections().enumerate() {
            for (l, (bindings, _)) in lines.iter().enumerate() {
                for (keys, command) in bindings.iter() {
                    let keys = parse_keys(keys).unwrap_or_else(|| panic!("the default keymap's {keys:?}"));
                    keymap.bind(layer, &keys, Bound::Command(*command), Shown::At(s, l));
                }
            }
        }
        keymap
    }
}

impl Keymap {
    /// The editor's keys, made once.
    pub fn standard() -> &'static Keymap {
        static STANDARD: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
        STANDARD.get_or_init(Keymap::default)
    }

    /// This keymap with the keys plugins' `views` declare (the plugin installed first
    /// first), each where it's free: one the editor has, or a view of another name
    /// before it, stays theirs, and that's kept as a [`Clash`]. Views of one name share
    /// their key: `focus_view` goes to the one for the file in front.
    pub fn with_views(mut self, views: &[PluginView]) -> Keymap {
        self.views = views.to_vec();
        self.clashes.clear();
        let mut given: Vec<(Vec<Key>, PluginView)> = vec![];
        for (i, view) in views.iter().enumerate() {
            let Some(said) = view.key else { continue };
            let Some(keys) = parse_keys(said) else { continue };
            if let Some(what) = self.editors_for(&keys) {
                self.clashes.push(Clash { view: *view, keys: said, with: Taken::Editor(what) });
                continue;
            }
            if let Some((_, other)) = given.iter().find(|(given, _)| *given == keys) {
                if other.id.name != view.id.name {
                    self.clashes.push(Clash { view: *view, keys: said, with: Taken::View(*other) });
                }
                continue;
            }
            let first = views.iter().position(|v| v.id.name == view.id.name).unwrap_or(i);
            self.bind(Layer::Of(ViewKind::Code), &keys, Bound::View(view.id.name), Shown::View(first));
            given.push((keys, *view));
        }
        self
    }

    /// What of the editor's `keys` would take, if they'd take any: a command bound to
    /// them or before them, or the keys they lead on to.
    fn editors_for(&self, keys: &[Key]) -> Option<String> {
        let code = self.layer(Layer::Of(ViewKind::Code));
        for end in 1..=keys.len() {
            match find(code, &keys[..end]) {
                Found::Bound(Bound::Command(command)) => return Some(command.name().to_string()),
                Found::Bound(Bound::Typed(line)) => return Some(format!(":{line}")),
                Found::Bound(Bound::View(_)) | Found::Nothing => return None,
                Found::Keys if end == keys.len() => return Some(format!("{} leads on to more", sequence(keys))),
                Found::Keys => {}
            }
        }
        None
    }

    /// The plugins' views it has, the plugin installed first first.
    pub fn views(&self) -> &[PluginView] {
        &self.views
    }

    /// The plugins' views whose keys are taken, and that yours haven't given one.
    pub fn clashes(&self) -> Vec<&Clash> {
        self.clashes.iter().filter(|clash| !self.has_key(clash.view.id.name)).collect()
    }

    /// Whether the views called `name` have a key, in the code's layer.
    fn has_key(&self, name: &str) -> bool {
        self.keys_of(name).next().is_some()
    }

    /// The keys that go to the views called `name`.
    fn keys_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Vec<Key>> + 'a {
        let mut found = vec![];
        walk(self.layer(Layer::Of(ViewKind::Code)), &mut vec![], &mut found);
        found.into_iter().filter(move |(_, binding)| matches!(binding.bound, Bound::View(of) if of == name)).map(|(keys, _)| keys)
    }

    /// The view called `name`: one of the editor's, or a plugin's.
    pub fn view_named(&self, name: &str) -> Option<ViewKind> {
        ViewKind::named(name).or_else(|| self.views.iter().find(|view| view.id.name == name).map(|view| view.kind()))
    }

    /// This keymap with your keys over it (the config's `"keys"`), and
    /// what's wrong with those that couldn't be bound.
    pub fn with_yours(self, yours: &[(String, String)]) -> (Keymap, Vec<String>) {
        self.with_yours_in(ViewKind::Code, yours)
    }

    /// The same, for `view`: the code's (which are every view's, under its own),
    /// or a view's own layer (the config's `"keys": { "files": { … } }`, or a plugin's
    /// view's by its name, `"goals"`).
    pub fn with_yours_in(mut self, view: ViewKind, yours: &[(String, String)]) -> (Keymap, Vec<String>) {
        let wrong = yours.iter().filter_map(|(keys, to)| self.bind_yours_in(view, keys, to).err()).collect();
        (self, wrong)
    }

    /// `keys` (`"space w"`) bound to `to` in the code: a `:` command (`":wa"`), a
    /// command's name (`"extend_line_below"`), a plugin's view (`"focus_view goals"`),
    /// or `no_op`, which unbinds them.
    pub fn bind_yours(&mut self, keys: &str, to: &str) -> Result<(), String> {
        self.bind_yours_in(ViewKind::Code, keys, to)
    }

    /// The same, in `view`'s layer: there `no_op` hides the code's binding too.
    pub fn bind_yours_in(&mut self, view: ViewKind, keys: &str, to: &str) -> Result<(), String> {
        let Some(sequence) = parse_keys(keys) else { return Err(format!("\"{keys}\" isn't keys (as in \"space w\", \"ctrl-s\")")) };
        // Keys that lead on to more (`space w`, to `space w v`) can't be bound themselves:
        // that would take those after them away. `no_op` takes them all away, as it says.
        let unbinding = !to.starts_with(':') && Command::named(to.trim()) == Some(NoOp);
        if !unbinding && let Found::Keys = self.find_in(view, &sequence) {
            let after: Vec<String> = self
                .bindings()
                .into_iter()
                .filter(|(of, more, _)| (*of == view || *of == ViewKind::Code) && more.starts_with(&sequence))
                .map(|(_, more, _)| self::sequence(&more))
                .collect();
            let shown = if after.len() > 4 { format!("{}, …", after[..4].join(", ")) } else { after.join(", ") };
            return Err(format!(
                "\"{keys}\": those keys lead on to others ({shown}), so they can't be bound themselves: bind keys after them, as \"{keys} x\""
            ));
        }
        let layer = yours_for(view);
        if let Some(line) = to.strip_prefix(':') {
            self.bind(layer, &sequence, Bound::Typed(line.trim().to_string()), Shown::Yours);
            return Ok(());
        }
        if let Some(name) = to.trim().strip_prefix("focus_view").filter(|rest| rest.is_empty() || rest.starts_with(' ')).map(str::trim) {
            let Some(at) = self.views.iter().position(|view| view.id.name == name) else {
                let mut names: Vec<&str> = self.views.iter().map(|view| view.id.name).collect();
                names.dedup();
                let there = if names.is_empty() { "no plugin has any".to_string() } else { format!("there's {}", names.join(", ")) };
                return Err(format!("\"{keys}\": no plugin has a view \"{name}\" ({there})"));
            };
            let name = self.views[at].id.name;
            self.bind(layer, &sequence, Bound::View(name), Shown::View(at));
            return Ok(());
        }
        match Command::named(to.trim()) {
            Some(NoOp) if view == ViewKind::Code => self.unbind(&sequence),
            Some(NoOp) => self.bind(layer, &sequence, Bound::Command(NoOp), Shown::Yours),
            // Where the command is in the cheatsheet already, it shows there.
            Some(command) => {
                let shown = self.shown_at(layer, command);
                self.bind(layer, &sequence, Bound::Command(command), shown)
            }
            None => return Err(format!("\"{keys}\": no command \"{to}\" (a : command starts with :, a plugin's view is focus_view and its name)")),
        }
        Ok(())
    }

    fn layer(&self, layer: Layer) -> &Trie {
        static NONE: Trie = Vec::new();
        self.layers.iter().find(|(of, _)| *of == layer).map_or(&NONE, |(_, trie)| trie)
    }

    fn layer_mut(&mut self, layer: Layer) -> &mut Trie {
        let at = match self.layers.iter().position(|(of, _)| *of == layer) {
            Some(at) => at,
            None => {
                self.layers.push((layer, vec![]));
                self.layers.len() - 1
            }
        };
        &mut self.layers[at].1
    }

    /// `keys` bound in `layer`, over whatever was: a binding on the way to them gives way.
    fn bind(&mut self, layer: Layer, keys: &[Key], bound: Bound, shown: Shown) {
        let Some((last, before)) = keys.split_last() else { return };
        let binding = Binding { bound, shown, order: self.made };
        self.made += 1;
        let mut nodes = self.layer_mut(layer);
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
        from(self.layer_mut(Layer::Of(ViewKind::Code)), keys);
    }

    /// What `keys` come to in the code.
    #[cfg(test)]
    pub(super) fn find(&self, keys: &[Key]) -> Found<'_> {
        self.find_in(ViewKind::Code, keys)
    }

    /// What `keys` come to in `view`: in its layers, or if they're nothing there, in the code's.
    pub(super) fn find_in(&self, view: ViewKind, keys: &[Key]) -> Found<'_> {
        for layer in layers_of(view) {
            match find(self.layer(layer), keys) {
                Found::Nothing => {}
                found => return found,
            }
        }
        Found::Nothing
    }

    /// Every binding (but those that hide one of the code's): the view it's in, its
    /// keys, and what they're bound to. A plugin's view's are those of what it shows,
    /// and yours for it, once for the views of each name.
    pub fn bindings(&self) -> Vec<(ViewKind, Vec<Key>, &Bound)> {
        self.all().into_iter().map(|(view, keys, binding)| (view, keys, &binding.bound)).collect()
    }

    /// The first of the plugins' views of each name.
    fn named_views(&self) -> Vec<PluginView> {
        let mut views: Vec<PluginView> = vec![];
        for view in &self.views {
            if views.iter().all(|seen| seen.id.name != view.id.name) {
                views.push(*view);
            }
        }
        views
    }

    fn all(&self) -> Vec<(ViewKind, Vec<Key>, &Binding)> {
        let shown = |binding: &&Binding| binding.bound != Bound::Command(NoOp);
        let mut all = vec![];
        for view in ViewKind::BUILT_IN {
            let mut theirs = vec![];
            walk(self.layer(Layer::Of(view)), &mut vec![], &mut theirs);
            all.extend(theirs.into_iter().filter(|(_, binding)| shown(binding)).map(|(keys, binding)| (view, keys, binding)));
        }
        for view in self.named_views() {
            let (yours, kind) = (self.layer(Layer::Named(view.id.name)), self.layer(Layer::Shows(view.id.shows)));
            let (mut theirs, mut under) = (vec![], vec![]);
            walk(yours, &mut vec![], &mut theirs);
            walk(kind, &mut vec![], &mut under);
            // What shows has, unless yours go over it.
            theirs.extend(under.into_iter().filter(|(keys, _)| matches!(find(yours, keys), Found::Nothing)));
            all.extend(theirs.into_iter().filter(|(_, binding)| shown(binding)).map(|(keys, binding)| (view.kind(), keys, binding)));
        }
        all
    }

    /// The `space ?` cheatsheet: every binding, on the lines [`DEFAULT`] and
    /// [`VIEWS`] put it on, and a section for the plugins' views of each name, under
    /// its title and key, with the lines of what it shows (keys of yours that have none
    /// at the end, under [`YOURS`]).
    pub fn cheatsheet(&self) -> Vec<Section> {
        let sections_of: Vec<_> = sections().collect();
        let named = self.named_views();
        // The plugins' views' sections come after the editor's views'.
        let after_views = DEFAULT.len() + VIEWS.len();
        let views_and_panes = DEFAULT.iter().position(|(title, _)| *title == VIEWS_AND_PANES).unwrap_or(0);
        let debugger_line = DEFAULT[views_and_panes].1.iter().position(|(bindings, _)| bindings.iter().any(|(_, c)| *c == Debugger)).unwrap_or(0);
        let group = |view: ViewKind| named.iter().position(|named| named.kind() == view).unwrap_or(0);
        // Where each goes: its section, which of the views' it's for, its line, and after which on it.
        let place = |view: ViewKind, shown: Shown| match shown {
            Shown::At(s, l) if matches!(sections_of[s].0, Layer::Shows(_)) => (after_views, group(view), l, 0),
            Shown::At(s, l) => (s, 0, l, 0),
            Shown::View(i) => (views_and_panes, 0, debugger_line, 1 + i),
            Shown::Yours => (usize::MAX, 0, 0, 0),
        };
        let mut all = self.all();
        all.sort_by_key(|(view, _, binding)| (place(*view, binding.shown), binding.order));
        let mut sections: Vec<Section> = vec![];
        let mut last = None;
        for (view, keys, binding) in all {
            let arg = match &binding.bound {
                Bound::Command(command) => command.arg(),
                Bound::View(_) | Bound::Typed(_) => None,
            };
            let at = place(view, binding.shown);
            let (title, what) = match (binding.shown, &binding.bound) {
                (Shown::At(s, l), _) if matches!(sections_of[s].0, Layer::Shows(_)) => (self.section_title(named[at.1]), sections_of[s].2[l].1.to_string()),
                (Shown::At(s, l), _) => (sections_of[s].1.to_string(), sections_of[s].2[l].1.to_string()),
                (Shown::View(i), _) => (VIEWS_AND_PANES.to_string(), going_to(self.views[i])),
                (Shown::Yours, Bound::Typed(line)) => (YOURS.to_string(), format!(":{line}")),
                (Shown::Yours, Bound::View(name)) => (YOURS.to_string(), format!("the {name} view")),
                (Shown::Yours, Bound::Command(command)) => (YOURS.to_string(), command.doc().to_string()),
            };
            // Yours in a view of its own say which.
            let what = if binding.shown == Shown::Yours && view != ViewKind::Code { format!("{}: {what}", view.name()) } else { what };
            if sections.last().is_none_or(|section| section.title != title) {
                sections.push(Section { title, lines: vec![] });
            }
            let lines = &mut sections.last_mut().expect("pushed just above").lines;
            match lines.last_mut() {
                Some(line) if last == Some(at) && binding.shown != Shown::Yours => {
                    line.keys.push(keys);
                    line.arg = line.arg.or(arg);
                }
                _ => lines.push(CheatLine { view, keys: vec![keys], arg, what }),
            }
            last = Some(at);
        }
        sections
    }

    /// The title of the section of the views called as `view` is: its title, and the keys
    /// that go there.
    fn section_title(&self, view: PluginView) -> String {
        let keys: Vec<String> = self.keys_of(view.id.name).map(|keys| sequence(&keys)).collect();
        if keys.is_empty() { view.title.to_string() } else { format!("{}  {}", view.title, keys.join("  ")) }
    }

    /// The line of the sections of `layer` `command` is first on, if it's on one: for a
    /// plugin's view's (yours for it), the lines of what it shows.
    fn shown_at(&self, layer: Layer, command: Command) -> Shown {
        let layer = match layer {
            Layer::Named(name) => match self.views.iter().find(|view| view.id.name == name) {
                Some(view) => Layer::Shows(view.id.shows),
                None => return Shown::Yours,
            },
            layer => layer,
        };
        for (s, (_, _, lines)) in sections().enumerate().filter(|(_, (of, ..))| *of == layer) {
            for (l, (bindings, _)) in lines.iter().enumerate() {
                if bindings.iter().any(|(_, c)| *c == command) {
                    return Shown::At(s, l);
                }
            }
        }
        Shown::Yours
    }
}

/// What the key of a plugin's `view` does, for the cheatsheet.
fn going_to(view: PluginView) -> String {
    let where_ = match view.place {
        super::ViewPlace::Tab => "a tab of this file's, opened if it isn't yet",
        super::ViewPlace::Beside => "beside this file",
    };
    format!("{}: {where_}; again: back to the code", view.title)
}

/// Every binding in `nodes`, after `keys`.
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

/// What `keys` come to in one layer. After a ctrl key of yours (`ctrl-k`), the next keys
/// can be had with ctrl held too: `ctrl-k ctrl-v` is `ctrl-k v`, as in helix.
fn find<'a>(layer: &'a Trie, keys: &[Key]) -> Found<'a> {
    let held = matches!(keys.first(), Some(Key::Ctrl(_)));
    let mut nodes = layer;
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
/// a char (`gg`, `]d`), else apart (`space f`, `space w v`).
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
    fn the_default_keymap_binds_each_key_once_in_each_view() {
        let mut seen = std::collections::HashSet::new();
        for (layer, _, lines) in sections() {
            for (bindings, _) in lines.iter() {
                for (keys, _) in bindings.iter() {
                    assert!(seen.insert((layer, self::keys(keys))), "{keys} twice in {layer:?}");
                }
            }
        }
        let of_views = |seen: &std::collections::HashSet<(Layer, Vec<Key>)>| seen.iter().filter(|(layer, _)| matches!(layer, Layer::Of(_))).count();
        assert_eq!(Keymap::standard().bindings().len(), of_views(&seen), "what plugins' views show has no keys of its own till there's one");
        let views: Vec<PluginView> = Shows::ALL
            .into_iter()
            .map(|shows| PluginView { id: super::super::ViewId { plugin: "p", name: shows.name(), shows }, title: "", place: shows.place(), key: None })
            .collect();
        assert_eq!(Keymap::default().with_views(&views).bindings().len(), seen.len());
    }

    #[test]
    fn prefixes_lead_on_and_ctrl_can_be_held() {
        let keymap = Keymap::standard();
        assert!(matches!(keymap.find(&keys("g")), Found::Keys));
        assert_eq!(command(keymap, "g g"), Some(GotoFileStart));
        assert_eq!(command(keymap, "space w v"), Some(Vsplit));
        assert_eq!(command(keymap, "ctrl-w"), None, "ctrl-w leads nowhere in normal mode: it's typing's");
        assert!(matches!(keymap.find(&keys("ctrl-w")), Found::Nothing));
        let mut yours = Keymap::default();
        yours.bind_yours("ctrl-k v", "vsplit").unwrap();
        assert_eq!(command(&yours, "ctrl-k ctrl-v"), Some(Vsplit), "as ctrl-k v");
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
            ("space W", ":wa"),
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
        assert!(matches!(keymap.find(&keys("space W")), Found::Bound(Bound::Typed(line)) if line == "wa"));
        assert_eq!(command(&keymap, "space f"), Some(FilePicker), "the rest of space is still there");
        assert!(matches!(keymap.find(&keys("space w")), Found::Keys), "space w leads on to the panes' keys, so it isn't bound");
        assert_eq!(command(&keymap, "space w v"), Some(Vsplit), "and they're as they were");
        assert!(matches!(keymap.find(&keys("g")), Found::Keys), "nor is g");
        assert_eq!(command(&keymap, "q"), Some(ReplayMacro), "a wrong name leaves the key as it was");
        assert_eq!(wrong.len(), 4, "{wrong:?}");
        assert!(wrong[0].starts_with("\"space w\": those keys lead on to others (space w v, space w w, space w q, space w o)"), "{}", wrong[0]);
        assert!(wrong[1].contains("\"g\": those keys lead on to others"), "{}", wrong[1]);
        assert!(wrong[2].contains("no command \"extend_lin_below\""), "{}", wrong[2]);
        assert!(wrong[3].contains("isn't keys"), "{}", wrong[3]);
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
        assert_eq!(sheet.last().map(|s| s.title.as_str()), Some(YOURS));
        assert_eq!(line(":w").as_deref(), Some("ctrl-s"));
        assert_eq!(line("find / till").as_deref(), Some("f t F T ‹c›"));
        assert_eq!(line("pick a file").as_deref(), Some("space f  space b"));
    }
}
