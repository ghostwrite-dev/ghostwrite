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

    /// All it's shown when it's asked about a server: what it's told, the environment, a
    /// program found on the PATH, and what's in the project.
    fn server_env(server: String, project: String, root: String) -> Vec<(String, String)> {
        let mut seen: Vec<(String, String)> = std::env::vars().collect();
        let listed = std::fs::read_dir("/project").map_or("nothing".into(), |files| files.count().to_string());
        let found = which("tool").unwrap_or_default();
        let elsewhere = which("../tool").unwrap_or_default();
        seen.extend(
            [("server", server), ("project", project), ("root", root), ("listed", listed), ("which", found), ("elsewhere", elsewhere)]
                .map(|(k, v)| (k.into(), v)),
        );
        seen
    }

    fn server_settings(server: String, _project: String, _root: String, section: String) -> Option<String> {
        match section.as_str() {
            "broken" => Some("{ not json".into()),
            _ => Some(format!(r#"{{ "server": "{server}", "section": "{section}" }}"#)),
        }
    }

    /// A panel that counts what it's told in its state, and shows the last answers.
    fn panel_open(language: String) -> Panel {
        Panel { state: "0".into(), blocks: vec![Block::Muted(format!("nothing yet, in {language}"))], working_on: None }
    }

    fn panel_answered(state: String, answers: Vec<Option<String>>) -> Panel {
        if answers.iter().flatten().any(|answer| answer.contains("panic")) {
            panic!("on purpose");
        }
        let told = state.parse::<u32>().unwrap_or(0) + 1;
        let said: Vec<String> = answers.into_iter().map(|answer| answer.unwrap_or_else(|| "none".into())).collect();
        let blocks =
            vec![Block::Heading(format!("told {told}")), Block::Code(CodeBlock { text: said.join("\n"), stands_out: Some('⊢') }), Block::Good("ok".into())];
        Panel { state: told.to_string(), blocks, working_on: None }
    }

    /// `{ "line": n }`: working on line n; `{}`: done, so ask again.
    fn panel_notification(state: String, method: String, params: String) -> (Panel, bool) {
        let line = params.split("\"line\":").nth(1).and_then(|rest| rest.trim_end_matches('}').trim().parse().ok());
        let blocks = vec![Block::Muted(method)];
        (Panel { state, blocks, working_on: line }, line.is_none())
    }
}

register!(Misbehaving);
