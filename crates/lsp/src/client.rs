//! One language server over stdio: JSON-RPC requests answered through
//! oneshot channels, notifications queued until the server is initialized,
//! and server requests answered so it never waits on us.

use std::collections::HashMap;
use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};

use crate::convert::{path_of, uri};

pub enum Event {
    Diagnostics { path: PathBuf, diagnostics: Vec<lsp_types::Diagnostic> },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    pub name: &'static str,
    writer: Arc<Mutex<ChildStdin>>,
    next_id: AtomicI64,
    pending: Pending,
    /// Messages wait in `queue` until the server has answered `initialize`.
    ready: Arc<AtomicBool>,
    queue: Arc<Mutex<Vec<Value>>>,
    capabilities: Arc<Mutex<Value>>,
    child: Mutex<Child>,
}

/// What a server is started with, and how it's answered.
pub struct Launch<'a> {
    pub name: &'static str,
    pub binary: PathBuf,
    pub args: &'a [&'a str],
    pub root: &'a Path,
    pub env: Vec<(String, String)>,
    /// Answers `workspace/configuration` for a section.
    pub settings: Box<dyn Fn(&str) -> Value + Send>,
}

impl Client {
    pub fn start(launch: Launch, events: mpsc::UnboundedSender<Event>) -> std::io::Result<Arc<Client>> {
        let mut child = Command::new(&launch.binary)
            .args(launch.args)
            .envs(launch.env)
            .current_dir(launch.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let client = Arc::new(Client {
            name: launch.name,
            writer: Arc::new(Mutex::new(stdin)),
            next_id: AtomicI64::new(1),
            pending: Arc::default(),
            ready: Arc::default(),
            queue: Arc::default(),
            capabilities: Arc::new(Mutex::new(Value::Null)),
            child: Mutex::new(child),
        });

        let (writer, pending, ready, queue, capabilities) =
            (client.writer.clone(), client.pending.clone(), client.ready.clone(), client.queue.clone(), client.capabilities.clone());
        let settings = launch.settings;
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                let id = message.get("id").and_then(Value::as_i64);
                let method = message.get("method").and_then(Value::as_str);
                match (id, method) {
                    // The answer to `initialize`: the server is ready for everything else.
                    (Some(0), None) => {
                        *capabilities.lock().unwrap() = message["result"]["capabilities"].clone();
                        write(&writer, &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                        // Pyright reads settings when told they changed.
                        write(&writer, &json!({ "jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": { "settings": {} } }));
                        let mut queue = queue.lock().unwrap();
                        ready.store(true, Ordering::SeqCst);
                        for queued in queue.drain(..) {
                            write(&writer, &queued);
                        }
                    }
                    (Some(id), None) => {
                        let result = match message.get("error") {
                            Some(e) => Err(e["message"].as_str().unwrap_or("language server error").to_string()),
                            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    (Some(_), Some(method)) => {
                        let result = match method {
                            "workspace/configuration" => Value::Array(
                                message["params"]["items"]
                                    .as_array()
                                    .map(|items| items.iter().map(|i| settings(i["section"].as_str().unwrap_or(""))).collect())
                                    .unwrap_or_default(),
                            ),
                            _ => Value::Null,
                        };
                        write(&writer, &json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }));
                    }
                    (None, Some("textDocument/publishDiagnostics")) => {
                        let params = &message["params"];
                        let path = params["uri"].as_str().and_then(path_of);
                        let diagnostics = serde_json::from_value(params["diagnostics"].clone()).unwrap_or_default();
                        if let Some(path) = path {
                            let _ = events.unbounded_send(Event::Diagnostics { path, diagnostics });
                        }
                    }
                    _ => {}
                }
            }
            // The server is gone: nobody will answer what's still pending.
            pending.lock().unwrap().clear();
        });

        let root = launch.root;
        let init = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": uri(root),
                "workspaceFolders": [{ "uri": uri(root), "name": root.file_name().map(|n| n.to_string_lossy()).unwrap_or_default() }],
                "capabilities": {
                    "general": { "positionEncodings": ["utf-16"] },
                    "textDocument": {
                        "synchronization": { "didSave": true },
                        "completion": { "completionItem": { "snippetSupport": false, "documentationFormat": ["plaintext", "markdown"] } },
                        "hover": { "contentFormat": ["plaintext", "markdown"] },
                        "definition": { "linkSupport": true },
                        "publishDiagnostics": { "relatedInformation": false },
                    },
                    "workspace": { "configuration": true, "workspaceFolders": true },
                },
            },
        });
        write(&client.writer, &init);
        Ok(client)
    }

    fn send(&self, message: Value) {
        if self.ready.load(Ordering::SeqCst) {
            return write(&self.writer, &message);
        }
        let mut queue = self.queue.lock().unwrap();
        // It may have become ready while we waited for the lock.
        if self.ready.load(Ordering::SeqCst) {
            drop(queue);
            write(&self.writer, &message);
        } else {
            queue.push(message);
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Result<Value, String>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx
    }

    /// Characters that open the completion menu, as the server announced them.
    pub fn trigger_characters(&self) -> Vec<String> {
        let caps = self.capabilities.lock().unwrap();
        caps["completionProvider"]["triggerCharacters"]
            .as_array()
            .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

fn write(writer: &Mutex<ChildStdin>, message: &Value) {
    let body = message.to_string();
    if let Ok(mut w) = writer.lock() {
        let _ = write!(w, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = w.flush();
    }
}

fn read_message(reader: &mut impl std::io::BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(n) = header.strip_prefix("Content-Length:") {
            length = n.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_framed_messages() {
        let mut input: &[u8] = b"Content-Length: 13\r\nContent-Type: x\r\n\r\n{\"id\": 4.0}  Content-Length: 2\r\n\r\n{}";
        let first = super::read_message(&mut input).unwrap();
        assert_eq!(first["id"], 4.0);
    }
}
