//! Claude through the Messages API, streamed.

use std::io::{BufRead as _, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::converse::{END_TOOL, Replier, Turn, add_turn, conversation};
use super::{Listener, ROOM, effort, tokens_in};
use crate::context::Request;
use crate::protocol;

pub(super) const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Claude through the Messages API.
pub(super) struct Chat {
    url: String,
    key: String,
    model: String,
    agent: ureq::Agent,
    messages: Vec<Value>,
}

impl Chat {
    pub(super) fn new(key: &str, model: &str, url: &str, req: &Request) -> Self {
        let agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(300))).build().into();
        Self { url: url.to_string(), key: key.to_string(), model: model.to_string(), agent, messages: conversation(req) }
    }
}

impl Replier for Chat {
    fn sees_files(&self) -> bool {
        true
    }

    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        add_turn(&mut self.messages, next);
        let body = json!({
            "model": self.model,
            "max_tokens": 32000,
            "stream": true,
            // Re-run a safety-classifier decline on Anthropic's recommended model for that category.
            "fallbacks": "default",
            "output_config": { "effort": effort() },
            "stop_sequences": [END_TOOL],
            // Cached: the system prompt (never changes), and the conversation
            // so far (each round and follow-up resends it).
            "system": [{ "type": "text", "text": protocol::system(), "cache_control": { "type": "ephemeral" } }],
            "cache_control": { "type": "ephemeral" },
            "messages": self.messages,
        });
        let response = self
            .agent
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2026-10-21")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .send_json(&body)
            .map_err(|e| format!("couldn't reach Claude: {e}"))?;
        let status = response.status();
        let mut reader = BufReader::new(response.into_body().into_reader());
        if !status.is_success() {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut reader, &mut text).ok();
            let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or(text);
            return Err(format!("Claude API {status}: {message}"));
        }
        let mut read = 0;
        for line in reader.lines() {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let line = line.map_err(|e| e.to_string())?;
            let Some(event) = line.strip_prefix("data:").and_then(|data| serde_json::from_str::<Value>(data.trim()).ok()) else { continue };
            if event["type"] == "message_start" {
                read = tokens_in(&event["message"]["usage"]);
            }
            if event["type"] == "message_delta"
                && let Some(written) = event["usage"]["output_tokens"].as_u64()
            {
                listener.used(read, written);
            }
            match event["type"].as_str() {
                Some("content_block_delta") if event["delta"]["type"] == "text_delta" => listener.text(event["delta"]["text"].as_str().unwrap_or_default()),
                Some("message_delta") if event["delta"]["stop_reason"] == "refusal" => return Err("Claude declined this request".into()),
                Some("error") => return Err(event["error"]["message"].as_str().unwrap_or("Claude API error").to_string()),
                _ => {}
            }
        }
        Ok(())
    }

    fn room(&self) -> usize {
        ROOM
    }
}

#[cfg(test)]
mod tests {
    use super::super::converse::converse;
    use super::super::tests::{Collect, DONE, asked_in, util_project, with_files};
    use super::*;
    use crate::protocol::SYSTEM;

    #[test]
    fn claude_sees_images_and_pdfs_it_reads_before_the_words_about_them() {
        let (_root, req) = with_files();
        let (url, served) = stand_in(vec![
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<tool>\nread figure.png\nread paper.pdf\nread util.py\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"stop_sequence"}}"#,
            ],
            &[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<done>\n</done>\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ],
        ]);
        let mut out = Collect::default();
        converse(&mut Chat::new("key", "a model", &url, &req), &req, &mut out, &AtomicBool::new(false)).unwrap();
        let requests = served.join().unwrap();
        let second = body_of(&requests[1]);
        let blocks = second["messages"].as_array().unwrap().last().unwrap()["content"].as_array().unwrap().clone();
        assert_eq!(blocks.iter().map(|block| block["type"].as_str().unwrap()).collect::<Vec<_>>(), ["image", "document", "text"]);
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["media_type"], "application/pdf");
        assert_eq!(blocks[1]["source"]["data"], "JVBERi0xLjQKdGhlIHBhcGVy");
        let words = blocks[2]["text"].as_str().unwrap();
        assert!(
            words.contains("figure.png is an image") && words.contains("paper.pdf is a PDF") && words.contains("def load"),
            "and a text file reads as ever: {words}"
        );
    }

    /// A server that answers a request for each of `rounds` with its events,
    /// as server-sent events, and hands back the requests it got.
    fn stand_in(rounds: Vec<&'static [&'static str]>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let mut requests = vec![];
            for events in rounds {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0; 1 << 20];
                let mut got = 0;
                // Headers, then as many body bytes as they say.
                loop {
                    got += stream.read(&mut request[got..]).unwrap();
                    let text = String::from_utf8_lossy(&request[..got]);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length =
                            text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap()));
                        if got >= end + 4 + length.unwrap_or(0) {
                            break;
                        }
                    }
                }
                let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
                write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                    .unwrap();
                requests.push(String::from_utf8_lossy(&request[..got]).into_owned());
            }
            requests
        });
        (url, served)
    }

    fn body_of(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[test]
    fn claude_through_the_api_uses_the_same_block() {
        let root = util_project("api-tools");
        let (url, served) = stand_in(vec![
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<tool>\nread util.py\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"stop_sequence"}}"#,
            ],
            &[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<done>\n</done>\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ],
        ]);
        let mut out = Collect::default();
        let req = asked_in(&root);
        converse(&mut Chat::new("key", "a model", &url, &req), &req, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.0, DONE);
        assert_eq!(out.1, ["reading util.py"]);
        let requests = served.join().unwrap();
        let first = body_of(&requests[0]);
        assert_eq!(first["stop_sequences"][0], "</tool>", "it stops at the block's end");
        assert!(first.get("tools").is_none());
        assert_eq!(first["system"][0]["text"], SYSTEM);
        let second = body_of(&requests[1]);
        assert!(second["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("def load"));
    }
}
