//! The jump list, as in helix: `ctrl-o` goes back to where a jump came
//! from, `ctrl-i` forward again. A jump is any move to another file, or
//! further than a screen in the same one (`gg`, `G`, `gd`, a search, `space j`).

use crate::document::DocId;

/// How many places are kept.
const KEPT: usize = 100;

#[derive(Default)]
pub struct Jumps {
    places: Vec<(DocId, usize)>,
    /// Where `ctrl-o` goes from: the place before this one. At the end, nothing's been gone back to.
    at: usize,
}

impl Jumps {
    /// A jump from `from`: it's where `ctrl-o` goes back to (what was ahead is dropped).
    pub fn jumped(&mut self, from: (DocId, usize)) {
        self.places.truncate(self.at);
        if self.places.last() != Some(&from) {
            self.places.push(from);
        }
        if self.places.len() > KEPT {
            self.places.remove(0);
        }
        self.at = self.places.len();
    }

    /// `ctrl-o`: `count` jumps back from `here` (if there are that many), and where that is.
    pub fn back(&mut self, count: usize, here: Option<(DocId, usize)>) -> Option<(DocId, usize)> {
        let to = self.at.checked_sub(count)?;
        // Leaving the end: remember it, for ctrl-i to come back to.
        if self.at == self.places.len()
            && let Some(here) = here
        {
            self.places.push(here);
        }
        self.at = to;
        Some(self.places[self.at])
    }

    /// `ctrl-i`: `count` jumps forward again (if there are that many), and where that is.
    pub fn forward(&mut self, count: usize) -> Option<(DocId, usize)> {
        if self.at + count >= self.places.len() {
            return None;
        }
        self.at += count;
        Some(self.places[self.at])
    }
}
