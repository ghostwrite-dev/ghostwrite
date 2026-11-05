//! The named commands of normal and select mode (and AI mode, which looks
//! around with them), as helix's `MappableCommand`s: what a key can do, by
//! name. The keymap says which keys run which (see [`super::keymap`]), and
//! `normal` what each does. Where helix has the same command, it has
//! helix's name.

/// Each command: its variant, its name, and what it does in a line.
macro_rules! commands {
    ($($variant:ident $name:literal $doc:literal,)*) => {
        /// A command a key can be bound to.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Command {
            $($variant,)*
        }

        impl Command {
            /// Every command.
            pub const ALL: &[Command] = &[$(Command::$variant,)*];

            /// Its name, as the config writes it: `extend_line_below`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Command::$variant => $name,)*
                }
            }

            /// What it does, in a line.
            pub fn doc(self) -> &'static str {
                match self {
                    $(Command::$variant => $doc,)*
                }
            }
        }
    };
}

commands! {
    NoOp "no_op" "nothing: a key bound to it is unbound",

    // Moving.
    MoveCharLeft "move_char_left" "left",
    MoveCharRight "move_char_right" "right",
    MoveLineDown "move_line_down" "down a line",
    MoveLineUp "move_line_up" "up a line",
    PageCursorHalfDown "page_cursor_half_down" "half a page down",
    PageCursorHalfUp "page_cursor_half_up" "half a page up",
    PageCursorDown "page_cursor_down" "a page down",
    PageCursorUp "page_cursor_up" "a page up",
    MoveNextWordStart "move_next_word_start" "to the next word",
    MovePrevWordStart "move_prev_word_start" "back to a word's start",
    MoveNextWordEnd "move_next_word_end" "to the word's end",
    MoveNextLongWordStart "move_next_long_word_start" "to the next WORD (words between spaces)",
    MovePrevLongWordStart "move_prev_long_word_start" "back to a WORD's start",
    MoveNextLongWordEnd "move_next_long_word_end" "to the WORD's end",
    GotoLine "goto_line" "to the line counted (10G); without a count, nowhere",
    GotoFileStart "goto_file_start" "to the file's start, or the line counted",
    GotoLastLine "goto_last_line" "to the last line",
    GotoLineStart "goto_line_start" "to the line's start",
    GotoLineEnd "goto_line_end" "to the line's end",
    GotoFirstNonwhitespace "goto_first_nonwhitespace" "to the line's first non-blank",
    GotoLastModification "goto_last_modification" "to where the last change was",
    GotoWindowTop "goto_window_top" "to the top line on screen",
    GotoWindowCenter "goto_window_center" "to the middle line on screen",
    GotoWindowBottom "goto_window_bottom" "to the bottom line on screen",
    FindNextChar "find_next_char" "select on to the next char typed (Enter: the line's end)",
    FindTillChar "find_till_char" "select on to just before the next char typed",
    FindPrevChar "find_prev_char" "select back to the char typed",
    TillPrevChar "till_prev_char" "select back to just after the char typed",
    RepeatLastMotion "repeat_last_motion" "the last f t mi ma ]p (and the like) again",
    GotoNextParagraph "goto_next_paragraph" "to the next paragraph",
    GotoPrevParagraph "goto_prev_paragraph" "to the paragraph before",
    GotoNextFunction "goto_next_function" "to the next function",
    GotoPrevFunction "goto_prev_function" "to the function before",
    GotoNextClass "goto_next_class" "to the next type",
    GotoPrevClass "goto_prev_class" "to the type before",
    GotoNextParameter "goto_next_parameter" "to the next argument",
    GotoPrevParameter "goto_prev_parameter" "to the argument before",
    GotoNextComment "goto_next_comment" "to the next comment",
    GotoPrevComment "goto_prev_comment" "to the comment before",
    GotoNextTest "goto_next_test" "to the next test",
    GotoPrevTest "goto_prev_test" "to the test before",
    GotoNextEntry "goto_next_entry" "to the next entry",
    GotoPrevEntry "goto_prev_entry" "to the entry before",
    GotoNextXmlElement "goto_next_xml_element" "to the next element",
    GotoPrevXmlElement "goto_prev_xml_element" "to the element before",
    ExpandSelection "expand_selection" "out to the syntax node around",
    ShrinkSelection "shrink_selection" "back in to the node inside",
    SelectNextSibling "select_next_sibling" "the next syntax node",
    SelectPrevSibling "select_prev_sibling" "the syntax node before",
    MoveParentNodeEnd "move_parent_node_end" "to the end of the node",
    MoveParentNodeStart "move_parent_node_start" "to the start of the node",
    MatchBrackets "match_brackets" "to the matching bracket",

    // Selecting.
    SelectMode "select_mode" "select mode, where motions extend (again: normal mode)",
    NormalMode "normal_mode" "normal mode",
    ExtendLineBelow "extend_line_below" "select the line (again: the next too)",
    ExtendToLineBounds "extend_to_line_bounds" "out to whole lines",
    ShrinkToLineBounds "shrink_to_line_bounds" "in to the whole lines",
    SelectAll "select_all" "everything",
    CollapseSelection "collapse_selection" "the selection down to its cursor",
    FlipSelections "flip_selections" "the cursor to the selection's other end",
    TrimSelections "trim_selections" "the selection without the spaces at its ends",
    SplitSelectionOnNewline "split_selection_on_newline" "a selection of each line",
    SelectAllSiblings "select_all_siblings" "a cursor on each syntax node beside this one",
    SelectAllChildren "select_all_children" "a cursor on each syntax node inside this one",
    SelectTextobjectAround "select_textobject_around" "select around the object typed",
    SelectTextobjectInner "select_textobject_inner" "select inside the object typed",
    CopySelectionOnNextLine "copy_selection_on_next_line" "copy the selection to the line below",
    CopySelectionOnPrevLine "copy_selection_on_prev_line" "copy the selection to the line above",
    SelectRegex "select_regex" "a cursor on each match in the selection",
    KeepPrimarySelection "keep_primary_selection" "back to one cursor",
    RemovePrimarySelection "remove_primary_selection" "drop the main cursor",
    RotateSelectionsBackward "rotate_selections_backward" "the main cursor: the one before",
    RotateSelectionsForward "rotate_selections_forward" "the main cursor: the one after",
    MergeSelections "merge_selections" "the selections merged into one",
    MergeConsecutiveSelections "merge_consecutive_selections" "the selections that touch merged",
    AlignSelections "align_selections" "the selections lined up in columns",

    // Editing.
    InsertMode "insert_mode" "type before the selection",
    AppendMode "append_mode" "type after the selection",
    InsertAtLineStart "insert_at_line_start" "type at the line's first non-blank",
    InsertAtLineEnd "insert_at_line_end" "type at the line's end",
    OpenBelow "open_below" "a new line below (3o: three, a cursor each)",
    OpenAbove "open_above" "a new line above",
    DeleteSelection "delete_selection" "delete the selection, yanked",
    DeleteSelectionNoyank "delete_selection_noyank" "delete the selection",
    ChangeSelection "change_selection" "type in place of the selection, yanked",
    ChangeSelectionNoyank "change_selection_noyank" "type in place of the selection",
    Yank "yank" "yank the selection",
    PasteAfter "paste_after" "paste after the selection",
    PasteBefore "paste_before" "paste before the selection",
    ReplaceWithYanked "replace_with_yanked" "the selection replaced with what was yanked",
    SelectRegister "select_register" "the register typed, for the next yank, delete or paste",
    Replace "replace" "each char of the selection replaced with the one typed",
    SwitchCase "switch_case" "switch the selection's case",
    SwitchToLowercase "switch_to_lowercase" "the selection in lower case",
    SwitchToUppercase "switch_to_uppercase" "the selection in upper case",
    Indent "indent" "indent the selection's lines",
    Unindent "unindent" "unindent the selection's lines",
    JoinSelections "join_selections" "join the selection's lines (or this and the next)",
    JoinSelectionsSpace "join_selections_space" "join lines, and select the spaces put between",
    AddNewlineBelow "add_newline_below" "a blank line below",
    AddNewlineAbove "add_newline_above" "a blank line above",
    Increment "increment" "the number here up by the count",
    Decrement "decrement" "the number here down by the count",
    ToggleComments "toggle_comments" "the selection's lines commented out, or back in",
    SurroundAdd "surround_add" "surround the selection with the pair typed",
    SurroundReplace "surround_replace" "the pair around typed first replaced with the one typed second",
    SurroundDelete "surround_delete" "delete the pair around typed",
    RepeatLastChange "repeat_last_change" "the last change again",
    RecordMacro "record_macro" "record keys (again: stop)",
    ReplayMacro "replay_macro" "play the keys recorded",
    Undo "undo" "undo",
    Redo "redo" "redo",

    // Searching.
    Search "search" "search the file, as it's typed",
    Rsearch "rsearch" "search the file backward",
    SearchNext "search_next" "the next match (select mode: add it)",
    SearchPrev "search_prev" "the match before (select mode: add it)",
    SearchSelectionDetectWordBounds "search_selection_detect_word_bounds" "search for the selection, as a word",
    SearchSelection "search_selection" "search for the selection, anywhere",

    // Beyond the buffer.
    CommandMode "command_mode" "the command line",
    JumpBackward "jump_backward" "back where a jump came from",
    JumpForward "jump_forward" "forward again",
    GotoNextBuffer "goto_next_buffer" "the next tab (4gn: four along)",
    GotoPreviousBuffer "goto_previous_buffer" "the tab before",
    MoveBufferNext "move_buffer_next" "move the tab along (4gN: four places)",
    MoveBufferPrevious "move_buffer_previous" "move the tab back",
    GotoDefinition "goto_definition" "to the definition",
    GotoReference "goto_reference" "through the references",
    GotoFile "goto_file" "the file named here (path:line:col)",
    GotoNextDiag "goto_next_diag" "the next diagnostic",
    GotoPrevDiag "goto_prev_diag" "the diagnostic before",
    AlignViewCenter "align_view_center" "scroll: this line to the middle",
    AlignViewTop "align_view_top" "scroll: this line to the top",
    AlignViewBottom "align_view_bottom" "scroll: this line to the bottom",
    Vsplit "vsplit" "two panes: this file's pages, or the next tab",
    RotateView "rotate_view" "the keys to the other pane",
    Wonly "wonly" "only this pane",
    FocusCode "focus_code" "back to the code (from select mode, normal mode first)",
    Ask "ask" "ask the model about the line or selection, or to change it",
    FileExplorer "file_explorer" "the file tree",
    Terminal "terminal" "the terminal: type in it",
    Debugger "debugger" "the debugger, while something's being debugged; from there, back to the code",
    FilePicker "file_picker" "pick a file (fuzzy)",
    BufferPicker "buffer_picker" "pick an open file",
    GlobalSearch "global_search" "search the project",
    Hover "hover" "the docs here",
    FileDiagnostics "file_diagnostics" "this file's diagnostics",
    RenameSymbol "rename_symbol" "rename what's here",
    CodeAction "code_action" "the language server's code actions here",
    SymbolPicker "symbol_picker" "pick one of the file's symbols",
    GotoNextProblem "goto_next_problem" "the next error (or warning); a file's pages follow",
    YankToClipboard "yank_to_clipboard" "yank the selection to the clipboard",
    PasteClipboardAfter "paste_clipboard_after" "paste the clipboard after the selection",
    PasteClipboardBefore "paste_clipboard_before" "paste the clipboard before the selection",
    Cheatsheet "cheatsheet" "every key",

    // In Files.
    FilesDown "files_down" "the row below",
    FilesUp "files_up" "the row above",
    FilesFirst "files_first" "the first row",
    FilesLast "files_last" "the last row",
    ExpandFolder "expand_folder" "open the folder, or preview the file",
    CollapseFolder "collapse_folder" "close the folder, or go up to the one it's in",
    OpenFile "open_file" "open the file (and go to the code), or open or close the folder",
    AddFile "add_file" "a new file or folder here (name/ for a folder)",
    RenameFile "rename_file" "rename the file or folder",
    DeleteFile "delete_file" "move the file or folder to the Trash",

    // In a plugin's view (pages, what's beside a file), and the debugger.
    ScrollDown "scroll_down" "scroll down",
    ScrollUp "scroll_up" "scroll up",
    ScrollPageDown "scroll_page_down" "scroll down a screen",
    ScrollPageUp "scroll_page_up" "scroll up a screen",
    ZoomIn "zoom_in" "zoom in",
    ZoomOut "zoom_out" "zoom out",
    ZoomReset "zoom_reset" "back to fitting the view",
}

impl Command {
    /// The command called `name`.
    pub fn named(name: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|command| command.name() == name)
    }

    /// Whether it waits for a char typed next (`f`, `r`, `mi`, `"`).
    pub fn takes_char(self) -> bool {
        use Command::*;
        matches!(
            self,
            FindNextChar
                | FindTillChar
                | FindPrevChar
                | TillPrevChar
                | Replace
                | SelectRegister
                | SurroundAdd
                | SurroundReplace
                | SurroundDelete
                | SelectTextobjectAround
                | SelectTextobjectInner
        )
    }

    /// What it waits for, as the cheatsheet writes it.
    pub fn arg(self) -> Option<&'static str> {
        use Command::*;
        Some(match self {
            SelectRegister => "‹r›",
            SurroundReplace => "‹a›‹b›",
            SelectTextobjectAround | SelectTextobjectInner => "‹o›",
            _ if self.takes_char() => "‹c›",
            _ => return None,
        })
    }

    /// Whether Enter and Tab are chars for it: a line break and a tab (`r`
    /// then Enter), or for `f` and `t`, Enter is the line's end.
    pub(super) fn takes_enter(self) -> bool {
        use Command::*;
        matches!(self, FindNextChar | FindTillChar | FindPrevChar | TillPrevChar | Replace)
    }

    /// Whether it's about the whole file or the editor, not a place in it,
    /// and so done once, not at each cursor.
    pub fn once(self) -> bool {
        use Command::*;
        matches!(self, Undo | Redo | RecordMacro | ReplayMacro | RepeatLastChange | JumpBackward | JumpForward)
    }

    /// Whether it waits until a review's done: in AI mode it does nothing.
    /// Looking around is free, starting something else isn't.
    pub(super) fn waits_for_review(self) -> bool {
        use Command::*;
        matches!(self, GotoNextProblem | FileDiagnostics | GlobalSearch | RenameSymbol | CodeAction | PasteClipboardBefore) || self.cursors()
    }

    /// Whether it works in any view, as it isn't about text: `:`, most under
    /// `space`, and going from view to view.
    pub fn anywhere(self) -> bool {
        use Command::*;
        matches!(self, CommandMode | Ask | Terminal | FileExplorer | GlobalSearch | FilePicker | BufferPicker | Cheatsheet | Debugger | FocusCode)
    }

    /// Whether it types (`i`, `a`, `o`, `c`…): on the terminal, it types in its shell.
    pub fn types(self) -> bool {
        use Command::*;
        matches!(self, InsertMode | AppendMode | InsertAtLineStart | InsertAtLineEnd | OpenBelow | OpenAbove | ChangeSelection | ChangeSelectionNoyank)
    }

    /// Whether it's about the cursors themselves, which the editor keeps.
    fn cursors(self) -> bool {
        use Command::*;
        matches!(
            self,
            CopySelectionOnNextLine
                | CopySelectionOnPrevLine
                | SelectRegex
                | KeepPrimarySelection
                | RemovePrimarySelection
                | RotateSelectionsBackward
                | RotateSelectionsForward
                | MergeSelections
                | MergeConsecutiveSelections
                | AlignSelections
        )
    }

    /// Whether the column `j` and `k` keep stays as it was: theirs, the
    /// syntax tree's moves, and the cursors'. Anything else forgets it.
    pub(super) fn keeps_column(self) -> bool {
        use Command::*;
        matches!(
            self,
            MoveLineDown
                | MoveLineUp
                | PageCursorHalfDown
                | PageCursorHalfUp
                | PageCursorDown
                | PageCursorUp
                | ExpandSelection
                | ShrinkSelection
                | SelectNextSibling
                | SelectPrevSibling
                | MoveParentNodeEnd
                | MoveParentNodeStart
        ) || self.cursors()
    }
}
