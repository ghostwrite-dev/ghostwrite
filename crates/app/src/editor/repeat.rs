//! `.` and macros: keys and typed text recorded, to play again. `.` plays
//! the last command that changed the text (`ciwfoo⎋`, `dd`, `>`), from
//! its first key back to normal mode; `Q` records keys until `Q` again,
//! and `q` plays them.

use ghostwrite_core::{Command, Key, Mode};
use gpui::{Context, Window};

use super::{DocId, Editor};
use ghostwrite_view::repeat::Recorded;
pub(super) use ghostwrite_view::repeat::Recorder;

impl Editor {
    /// The file and version the cursor's in.
    fn version(&self) -> Option<(DocId, u64)> {
        self.tab().map(|t| (t.doc.id, t.doc.buffer.version()))
    }

    /// Before a key, which runs `command` if any: record it (a command
    /// starts in normal mode with nothing typed toward it).
    pub(super) fn record_key(&mut self, key: Key, command: Option<Command>) {
        let at_rest = matches!(self.modal.mode, Mode::Normal | Mode::Select) && self.modal.pending_label().is_empty();
        let now = self.version();
        self.recorder.key(key, command, at_rest, now);
    }

    /// After a key, which ran `command` if any: a finished command that
    /// changed the text is what `.` plays.
    pub(super) fn recorded_key(&mut self, command: Option<Command>) {
        let done = matches!(self.modal.mode, Mode::Normal | Mode::Select) && self.modal.pending_label().is_empty();
        let now = self.version();
        self.recorder.keyed(command, done, now);
    }

    /// Text typed in insert mode, as part of the command and any macro.
    pub(super) fn record_text(&mut self, text: &str) {
        self.recorder.text(text);
    }

    /// `.`: the last change again, here.
    pub(super) fn repeat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.recorder.last_change().to_vec();
        if keys.is_empty() {
            return self.project.say("nothing to repeat yet");
        }
        self.play(&keys, window, cx);
    }

    /// `Q`: start recording a macro, or stop.
    pub(super) fn record_macro(&mut self) {
        match self.recorder.record_macro() {
            Some(n) => self.project.say(format!("recorded {n} keys: q plays them")),
            None => self.project.say("recording: Q stops"),
        }
    }

    /// `q`: the recorded macro, played.
    pub(super) fn play_macro(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recorder.recording() {
            return self.project.say("still recording: Q stops first");
        }
        let keys = self.recorder.last_macro().to_vec();
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
        self.recorder.recording()
    }
}
