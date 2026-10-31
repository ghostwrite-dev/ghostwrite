//! A server that crashes is noticed: what's pending fails, nothing hangs,
//! and the editor hears why. The server is a few lines of shell.

use std::time::{Duration, Instant};

use futures::channel::mpsc;
use ghostwrite_lsp::client::{Client, Event, Launch};

#[test]
fn a_server_that_crashes_is_noticed_and_says_why() {
    // Answers `initialize`, complains on stderr, and exits.
    let script = r#"
        body='{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}'
        printf 'Content-Length: %d\r\n\r\n%s' ${#body} "$body"
        sleep 0.3
        echo "panicked at 'index out of bounds'" >&2
        exit 101
    "#;
    let (tx, mut events) = mpsc::unbounded();
    let root = std::env::temp_dir();
    let launch =
        Launch { name: "crashy", binary: "/bin/sh".into(), args: &["-c", script], root: &root, env: vec![], settings: Box::new(|_| serde_json::Value::Null) };
    let client = Client::start(launch, tx).unwrap();
    let mut asked = client.request("textDocument/hover", serde_json::json!({}));

    let start = Instant::now();
    let why = loop {
        if let Ok(Event::Exited { server, why }) = events.try_recv() {
            assert_eq!(server, "crashy");
            break why;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never noticed");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(why.contains("index out of bounds"), "{why}");
    assert!(!client.alive());
    assert!(!matches!(asked.try_recv(), Ok(None)), "what was pending fails rather than waiting forever");
    let mut later = client.request("textDocument/hover", serde_json::json!({}));
    assert!(matches!(later.try_recv(), Ok(Some(Err(_)))), "and so does anything asked after");
}
