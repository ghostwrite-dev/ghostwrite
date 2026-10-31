//! `.` and macros: keys and typed text recorded, to play again. `.` plays
//! the last command that changed the text (`ciwfoo⎋`, `dd`, `>`), from
//! its first key back to normal mode; `Q` records keys until `Q` again,
//! and `q` plays them.

use ghostwrite_core::{Key, Mode};
use gpui::{Context, Window};

use super::{DocId, Editor};

/// A key, or text typed in insert mode.
#[derive(Debug, Clone)]
pub(super) enum Recorded {
    Key(Key),
    Text(String),
}

#[derive(Default)]
pub(super) struct Recorder {
    /// The command being typed, and the file and its version as it began.
    change: Vec<Recorded>,
    began: Option<(DocId, u64)>,
    /// The last command that changed the text: `.` plays it.
    last_change: Vec<Recorded>,
    /// Keys recorded for a macro (`Q` … `Q`), and the last macro.
    recording: Option<Vec<Recorded>>,
    last_macro: Vec<Recorded>,
    /// Playing keys back: they aren't recorded again.
    playing: bool,
}

impl Editor {
    /// The file and version the cursor's in.
    fn version(&self) -> Option<(DocId, u64)> {
        self.tab().map(|t| (t.doc.id, t.doc.buffer.version()))
    }

    /// Before a key: record it (a command starts in normal mode with nothing typed toward it).
    pub(super) fn record_key(&mut self, key: Key) {
        if self.recorder.playing {
            return;
        }
        if matches!(self.modal.mode, Mode::Normal | Mode::Select) && self.modal.pending_label().is_empty() {
            self.recorder.change.clear();
            self.recorder.began = self.version();
        }
        self.recorder.change.push(Recorded::Key(key));
        if let Some(keys) = self.recorder.recording.as_mut().filter(|_| key != Key::Char('Q')) {
            keys.push(Recorded::Key(key));
        }
    }

    /// After a key: a finished command that changed the text is what `.` plays.
    pub(super) fn recorded_key(&mut self, key: Key) {
        let done = matches!(self.modal.mode, Mode::Normal | Mode::Select) && self.modal.pending_label().is_empty();
        let changed = self.recorder.began.is_some_and(|(doc, version)| self.version().is_some_and(|(d, v)| d == doc && v != version));
        if !self.recorder.playing && done && changed && key != Key::Char('.') {
            self.recorder.last_change = std::mem::take(&mut self.recorder.change);
            self.recorder.began = None;
        }
    }

    /// Text typed in insert mode, as part of the command and any macro.
    pub(super) fn record_text(&mut self, text: &str) {
        if self.recorder.playing {
            return;
        }
        self.recorder.change.push(Recorded::Text(text.to_string()));
        if let Some(keys) = self.recorder.recording.as_mut() {
            keys.push(Recorded::Text(text.to_string()));
        }
    }

    /// `.`: the last change again, here.
    pub(super) fn repeat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.recorder.last_change.clone();
        if keys.is_empty() {
            return self.project.say("nothing to repeat yet");
        }
        self.play(&keys, window, cx);
    }

    /// `Q`: start recording a macro, or stop.
    pub(super) fn record_macro(&mut self) {
        match self.recorder.recording.take() {
            Some(keys) => {
                self.project.say(format!("recorded {} keys: q plays them", keys.len()));
                self.recorder.last_macro = keys;
            }
            None => {
                self.recorder.recording = Some(vec![]);
                self.project.say("recording: Q stops");
            }
        }
    }

    /// `q`: the recorded macro, played.
    pub(super) fn play_macro(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recorder.recording.is_some() {
            return self.project.say("still recording: Q stops first");
        }
        let keys = self.recorder.last_macro.clone();
        if keys.is_empty() {
            return self.project.say("no macro yet: Q records one");
        }
        self.play(&keys, window, cx);
    }

    fn play(&mut self, keys: &[Recorded], window: &mut Window, cx: &mut Context<Self>) {
        self.recorder.playing = true;
        for key in keys {
            match key {
                Recorded::Key(key) => {
                    self.mode_key(*key, window, cx);
                }
                Recorded::Text(text) => self.type_text(text, cx),
            }
        }
        self.recorder.playing = false;
        cx.notify();
    }

    /// Whether a macro's being recorded (the status line says so).
    pub(super) fn recording_macro(&self) -> bool {
        self.recorder.recording.is_some()
    }
}
