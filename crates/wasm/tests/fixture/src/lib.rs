//! Each command tries something a plugin shouldn't be able to do to the editor.

use ghostwrite_plugin_api::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static CALLS: AtomicUsize = AtomicUsize::new(0);

struct Misbehaving;

impl Plugin for Misbehaving {
    fn run(name: String, args: Vec<String>, _context: Context) -> Reply {
        let calls = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        match name.as_str() {
            "calls" => Ok(vec![Action::Say(calls.to_string())]),
            "spin" => loop {
                std::hint::spin_loop();
            },
            "panic" => panic!("on purpose"),
            "hoard" => {
                let mut kept = vec![];
                loop {
                    kept.push(vec![1u8; 16 << 20]);
                    std::hint::black_box(&kept);
                }
            }
            "read" => std::fs::read_to_string(&args[0]).map(|text| vec![Action::Say(text)]).map_err(|e| e.to_string()),
            "write" => std::fs::write(&args[0], "written").map(|()| vec![]).map_err(|e| e.to_string()),
            "list" => std::fs::read_dir(&args[0]).map(|_| vec![Action::Say("listed".into())]).map_err(|e| e.to_string()),
            "environment" => Ok(vec![Action::Say(std::env::vars().count().to_string())]),
            "connect" => std::net::TcpStream::connect(&args[0]).map(|_| vec![Action::Say("connected".into())]).map_err(|e| e.to_string()),
            other => Err(format!("no command {other}")),
        }
    }
}

register!(Misbehaving);
