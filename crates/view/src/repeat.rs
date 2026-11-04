//! `.` and macros: keys and typed text recorded, to play again. `.` plays
//! the last command that changed the text (`ciwfoo⎋`, `dd`, `>`), from
//! its first key back to normal mode; `Q` records keys until `Q` again,
//! and `q` plays them. This is the recorder; the app plays them.

use ghostwrite_core::{Command, Key};

use crate::document::DocId;

/// A key, or text typed in insert mode.
#[derive(Debug, Clone)]
pub enum Recorded {
    Key(Key),
    Text(String),
}

#[derive(Default)]
pub struct Recorder {
    /// The command being typed, and the file and its version as it began.
    change: Vec<Recorded>,
    began: Option<(DocId, u64)>,
    /// The last command that changed the text: `.` plays it.
    last_change: Vec<Recorded>,
    /// Keys recorded for a macro (`Q` … `Q`), and the last macro.
    recording: Option<Vec<Recorded>>,
    last_macro: Vec<Recorded>,
    /// Playing keys back: they aren't recorded again.
    pub playing: bool,
}

impl Recorder {
    /// Before a key, which runs `command` if any: record it. `at_rest`: a
    /// command starts with it (in normal mode, with nothing typed toward
    /// one), in the file and version `now`.
    pub fn key(&mut self, key: Key, command: Option<Command>, at_rest: bool, now: Option<(DocId, u64)>) {
        if self.playing {
            return;
        }
        if at_rest {
            self.change.clear();
            self.began = now;
        }
        self.change.push(Recorded::Key(key));
        // The key that stops recording isn't recorded.
        if let Some(keys) = self.recording.as_mut().filter(|_| command != Some(Command::RecordMacro)) {
            keys.push(Recorded::Key(key));
        }
    }

    /// After a key, which ran `command` if any: a finished command (`done`)
    /// that changed the text (it's in the file and version `now`) is what
    /// `.` plays, unless it was `.` itself.
    pub fn keyed(&mut self, command: Option<Command>, done: bool, now: Option<(DocId, u64)>) {
        let changed = self.began.is_some_and(|(doc, version)| now.is_some_and(|(d, v)| d == doc && v != version));
        if !self.playing && done && changed && command != Some(Command::RepeatLastChange) {
            self.last_change = std::mem::take(&mut self.change);
            self.began = None;
        }
    }

    /// Text typed in insert mode, as part of the command and any macro.
    pub fn text(&mut self, text: &str) {
        if self.playing {
            return;
        }
        self.change.push(Recorded::Text(text.to_string()));
        if let Some(keys) = self.recording.as_mut() {
            keys.push(Recorded::Text(text.to_string()));
        }
    }

    /// What `.` plays.
    pub fn last_change(&self) -> &[Recorded] {
        &self.last_change
    }

    /// What `q` plays.
    pub fn last_macro(&self) -> &[Recorded] {
        &self.last_macro
    }

    /// `Q`: start recording a macro, or stop: how many keys it has, if it stopped.
    pub fn record_macro(&mut self) -> Option<usize> {
        match self.recording.take() {
            Some(keys) => {
                let n = keys.len();
                self.last_macro = keys;
                Some(n)
            }
            None => {
                self.recording = Some(vec![]);
                None
            }
        }
    }

    /// Whether a macro's being recorded.
    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }
}
