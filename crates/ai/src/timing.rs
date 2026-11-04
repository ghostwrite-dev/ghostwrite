//! How long the model keeps the user waiting, kept (in Ghostwrite's support
//! folder) to see where the time goes: for each wait, how long until the
//! first of the reply came, and until there was something to look at; and
//! whether the model's process had been started ahead of time. `:timings`
//! sums them up.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ghostwrite_project::store;
use serde_json::{Value, json};

const FILE: &str = "timings.jsonl";
/// Waits kept: enough to see a trend.
const KEPT: usize = 500;
/// Waits of each kind summed up.
const SUMMED: usize = 20;

/// What the user is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// The first step, after asking.
    Ask,
    /// A step after saying something during the review.
    FollowUp,
    /// The next step, after Tab.
    NextStep,
}

impl Wait {
    const ALL: [Wait; 3] = [Wait::Ask, Wait::FollowUp, Wait::NextStep];

    fn name(self) -> &'static str {
        match self {
            Wait::Ask => "ask",
            Wait::FollowUp => "follow-up",
            Wait::NextStep => "next step",
        }
    }
}

/// A wait being timed.
#[derive(Debug)]
pub struct Timer {
    wait: Wait,
    since: Instant,
    first_text: Option<Duration>,
    ahead: Option<bool>,
}

impl Timer {
    pub fn start(wait: Wait) -> Self {
        Self { wait, since: Instant::now(), first_text: None, ahead: None }
    }

    pub fn wait(&self) -> Wait {
        self.wait
    }

    /// The reply's first text came.
    pub fn text(&mut self) {
        self.first_text.get_or_insert_with(|| self.since.elapsed());
    }

    /// The model's process was up: started ahead of time, or not.
    pub fn started(&mut self, ahead: bool) {
        self.ahead.get_or_insert(ahead);
    }

    /// There's something to look at: keep how long it took, for `model`.
    pub fn shown(self, model: &str) {
        let ms = |d: Duration| d.as_millis() as u64;
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let entry = json!({
            "at": at,
            "model": model,
            "wait": self.wait.name(),
            "first_text_ms": self.first_text.map(ms),
            "shown_ms": ms(self.since.elapsed()),
            "started_ahead": self.ahead,
        });
        let mut entries = store::lines(FILE);
        entries.push(entry.to_string());
        store::write_lines(FILE, &entries, KEPT);
    }
}

/// The recent waits of each kind, in a line: the median times.
pub fn summary() -> String {
    summarize(&store::lines(FILE).iter().filter_map(|line| serde_json::from_str(line).ok()).collect::<Vec<Value>>())
}

fn summarize(entries: &[Value]) -> String {
    let secs = |ms: u64| format!("{:.1}s", ms as f64 / 1000.);
    let mut said = vec![];
    for wait in Wait::ALL {
        let recent: Vec<&Value> = entries.iter().filter(|e| e["wait"] == wait.name()).collect();
        let recent = &recent[recent.len().saturating_sub(SUMMED)..];
        if recent.is_empty() {
            continue;
        }
        let shown = median(recent.iter().filter_map(|e| e["shown_ms"].as_u64()).collect());
        let mut line = format!("{} ({}): shown in {}", wait.name(), recent.len(), shown.map(secs).unwrap_or_default());
        if let Some(first) = median(recent.iter().filter_map(|e| e["first_text_ms"].as_u64()).collect()) {
            line.push_str(&format!(", first text in {}", secs(first)));
        }
        said.push(line);
    }
    if said.is_empty() { "nothing timed yet: ask something first".into() } else { format!("medians · {}", said.join(" · ")) }
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    values.get(values.len() / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_are_summed_up_by_kind() {
        let entry = |wait: &str, first: Option<u64>, shown: u64| json!({ "wait": wait, "first_text_ms": first, "shown_ms": shown });
        let entries = [entry("ask", Some(900), 3000), entry("ask", Some(1100), 2000), entry("ask", Some(700), 4000), entry("next step", None, 0)];
        assert_eq!(summarize(&entries), "medians · ask (3): shown in 3.0s, first text in 0.9s · next step (1): shown in 0.0s");
        assert_eq!(summarize(&[]), "nothing timed yet: ask something first");
    }

    #[test]
    fn a_timer_keeps_the_first_text_and_start() {
        let mut timer = Timer::start(Wait::Ask);
        timer.text();
        let first = timer.first_text;
        std::thread::sleep(Duration::from_millis(2));
        timer.text();
        timer.started(true);
        timer.started(false);
        assert_eq!((timer.first_text, timer.ahead), (first, Some(true)), "the first of each");
    }
}
