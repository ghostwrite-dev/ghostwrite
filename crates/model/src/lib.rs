//! A local model, run with llama.cpp, linked in: nothing to install, no server,
//! no port. It runs in a process of its own, this same program started by the
//! app (`--model-worker`), because llama.cpp can abort on a failure, which mustn't
//! take the user's unsaved work with it, and because ending a process is the one
//! sure way to give back the memory.
//!
//! The app and the worker talk in lines of JSON over the worker's stdin and
//! stdout. Asked with `{"id", "messages": [{"role", "content"}…], "grammar",
//! "temperature", "max_tokens"}`, or `{"cancel": id}` to stop, it answers with any number of
//! `{"status"}` and `{"text"}`, then `{"done": true}` or `{"error"}`. `{"fatal"}` means
//! it's ending.
//!
//! - It ends when its stdin closes, which is when the app ends, however it ends.
//! - It ends after a while idle, so the model's memory is only held while it's used.
//! - There's only ever one, across every running Ghostwrite ([`lock`]).
//! - It won't load what doesn't fit, and ends if memory runs out later ([`memory`]).

#[cfg(feature = "llama")]
mod fetch;
#[cfg(feature = "llama")]
mod generate;
pub mod memory;

#[cfg(feature = "llama")]
use std::io::BufRead as _;
use std::io::Write as _;
#[cfg(feature = "llama")]
use std::sync::Arc;
#[cfg(feature = "llama")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "llama")]
use std::sync::mpsc::{self, RecvTimeoutError};
#[cfg(feature = "llama")]
use std::time::Duration;

use serde_json::{Value, json};

/// Idle this long, the worker ends; the next request starts another.
#[cfg(feature = "llama")]
const IDLE: Duration = Duration::from_secs(300);

/// There's no worker in this build.
#[cfg(not(feature = "llama"))]
pub fn worker(_model: &str) -> ! {
    fatal("this build of Ghostwrite was made without local models");
}

/// Be the worker for `model`, until it's time to end.
#[cfg(feature = "llama")]
pub fn worker(model: &str) -> ! {
    let _lock = lock().unwrap_or_else(|why| fatal(&why));
    let cancel = Arc::new(AtomicU64::new(0));
    let (requests, incoming) = mpsc::channel::<Value>();
    let cancelling = cancel.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(id) = message["cancel"].as_u64() {
                cancelling.store(id, Ordering::SeqCst);
            } else if requests.send(message).is_err() {
                break;
            }
        }
        // The app's gone: at once, even in the middle of a reply.
        std::process::exit(0);
    });

    let path = fetch::path_of(model, &mut |status| say(&json!({ "status": status }))).unwrap_or_else(|why| fatal(&why));
    memory::check_fits(&path).unwrap_or_else(|why| fatal(&why));
    say(&json!({ "status": "loading the model" }));
    memory::guard(|why| fatal(why));
    let mut model = generate::Model::load(&path).unwrap_or_else(|why| fatal(&why));

    loop {
        let request = match incoming.recv_timeout(IDLE) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => std::process::exit(0),
        };
        match model.reply(&request, &cancel, &mut |text| say(&json!({ "text": text }))) {
            Ok(()) => say(&json!({ "done": true })),
            Err(why) => say(&json!({ "error": why })),
        }
    }
}

fn say(message: &Value) {
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{message}").and_then(|()| out.flush()).is_err() {
        std::process::exit(0);
    }
}

fn fatal(why: &str) -> ! {
    say(&json!({ "fatal": why }));
    std::process::exit(1);
}

/// One worker at a time: two models would be twice the memory, which can panic
/// the machine. The system lets go of the lock when the process ends.
#[cfg(feature = "llama")]
fn lock() -> Result<std::fs::File, String> {
    let path = std::env::temp_dir().join("ghostwrite-model.lock");
    let file = std::fs::File::create(&path).map_err(|e| format!("couldn't make {}: {e}", path.display()))?;
    if file.try_lock().is_err() {
        return Err("another local model is already running, so this one can't load too: quit whatever is using it, or wait for it to finish".into());
    }
    Ok(file)
}
