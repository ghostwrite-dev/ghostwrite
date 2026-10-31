//! One language server over stdio: JSON-RPC requests answered through
//! oneshot channels, notifications queued until the server is initialized,
//! and server requests answered so it never waits on us. Everything to the
//! server goes through a writer thread, so neither the UI nor the reader
//! ever waits on it. A server that exits is noticed: what's pending fails,
//! nothing more is sent, and an [`Event::Exited`] says why (the end of what
//! it wrote to stderr).

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};

use crate::convert::{path_of, uri};

pub enum Event {
    Diagnostics {
        path: PathBuf,
        /// The document version they're for, if the server says.
        version: Option<i32>,
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
    /// The server wants `path` shown, at `selection` (`window/showDocument`).
    Show { path: PathBuf, selection: Option<lsp_types::Range> },
    /// Lean's `$/lean/fileProgress`: the first line of `path` it's still
    /// checking, or none once it's done.
    Progress { path: PathBuf, checking: Option<u32> },
    /// The server's semantic tokens changed (`workspace/semanticTokens/refresh`):
    /// ask it again, for each of its files.
    Recolor { server: &'static str },
    /// The server wants these edits made (`workspace/applyEdit`, after a code action's command).
    ApplyEdit { edit: Value },
    /// The server exited (or crashed); `why` is the end of its stderr.
    Exited { server: &'static str, why: String },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

/// How many lines of a server's stderr are kept, to say why it stopped.
const STDERR_LINES: usize = 40;

pub struct Client {
    pub name: &'static str,
    /// To the writer thread.
    outbox: std::sync::mpsc::Sender<Value>,
    next_id: AtomicI64,
    pending: Pending,
    /// Messages wait in `queue` until the server has answered `initialize`.
    ready: Arc<AtomicBool>,
    queue: Arc<Mutex<Vec<Value>>>,
    /// Until the server exits.
    alive: Arc<AtomicBool>,
    capabilities: Arc<Mutex<Value>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    child: Mutex<Option<Child>>,
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
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| std::io::Error::other("no stderr"))?;
        let (outbox, to_server) = std::sync::mpsc::channel::<Value>();
        std::thread::spawn(move || {
            for message in to_server {
                if write(&mut stdin, &message).is_err() {
                    break;
                }
            }
        });
        let client = Arc::new(Client {
            name: launch.name,
            outbox: outbox.clone(),
            next_id: AtomicI64::new(1),
            pending: Arc::default(),
            ready: Arc::default(),
            queue: Arc::default(),
            alive: Arc::new(AtomicBool::new(true)),
            capabilities: Arc::new(Mutex::new(Value::Null)),
            stderr: Arc::default(),
            child: Mutex::new(Some(child)),
        });

        let lines = client.stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut lines = lines.lock().unwrap();
                if lines.len() == STDERR_LINES {
                    lines.pop_front();
                }
                lines.push_back(line);
            }
        });

        let (pending, ready, queue, capabilities, alive, lines) =
            (client.pending.clone(), client.ready.clone(), client.queue.clone(), client.capabilities.clone(), client.alive.clone(), client.stderr.clone());
        let (settings, name) = (launch.settings, launch.name);
        let send = move |message: Value| {
            let _ = outbox.send(message);
        };
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = match read_message(&mut reader) {
                    Read::Message(message) => message,
                    // Skipped: the next one may be fine.
                    Read::Garbled => continue,
                    Read::End => break,
                };
                // Our requests' ids are numbers; a server's may be anything (Lean's are strings too).
                let id = message.get("id").filter(|id| !id.is_null()).map(|id| id.as_i64().unwrap_or(-1));
                let method = message.get("method").and_then(Value::as_str);
                match (id, method) {
                    // The answer to `initialize`: the server is ready for everything else.
                    (Some(0), None) => {
                        *capabilities.lock().unwrap() = message["result"]["capabilities"].clone();
                        send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                        // Pyright reads settings when told they changed.
                        send(json!({ "jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": { "settings": {} } }));
                        let mut queue = queue.lock().unwrap();
                        ready.store(true, Ordering::SeqCst);
                        for queued in queue.drain(..) {
                            send(queued);
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
                        let params = &message["params"];
                        let result = match method {
                            "workspace/configuration" => Ok(Value::Array(
                                params["items"]
                                    .as_array()
                                    .map(|items| items.iter().map(|i| settings(i["section"].as_str().unwrap_or(""))).collect())
                                    .unwrap_or_default(),
                            )),
                            "workspace/semanticTokens/refresh" => {
                                let _ = events.unbounded_send(Event::Recolor { server: name });
                                Ok(Value::Null)
                            }
                            "workspace/applyEdit" => {
                                let _ = events.unbounded_send(Event::ApplyEdit { edit: params["edit"].clone() });
                                Ok(json!({ "applied": true }))
                            }
                            "window/showDocument" => {
                                let path = params["uri"].as_str().and_then(path_of).filter(|_| params["external"] != true);
                                let selection = serde_json::from_value(params["selection"].clone()).ok();
                                let shown = path.is_some_and(|path| events.unbounded_send(Event::Show { path, selection }).is_ok());
                                Ok(json!({ "success": shown }))
                            }
                            // Fine to say yes to, and nothing to do.
                            "client/registerCapability"
                            | "client/unregisterCapability"
                            | "window/workDoneProgress/create"
                            | "workspace/inlayHint/refresh"
                            | "workspace/codeLens/refresh"
                            | "workspace/diagnostic/refresh" => Ok(Value::Null),
                            // Anything else (`workspace/applyEdit`, …) we don't do, and say so.
                            _ => Err(json!({ "code": -32601, "message": format!("{method} isn't supported") })),
                        };
                        send(match result {
                            Ok(result) => json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }),
                            Err(error) => json!({ "jsonrpc": "2.0", "id": message["id"], "error": error }),
                        });
                    }
                    (None, Some("textDocument/publishDiagnostics")) => {
                        let params = &message["params"];
                        let path = params["uri"].as_str().and_then(path_of);
                        let version = params["version"].as_i64().map(|v| v as i32);
                        let diagnostics = serde_json::from_value(params["diagnostics"].clone()).unwrap_or_default();
                        if let Some(path) = path {
                            let _ = events.unbounded_send(Event::Diagnostics { path, version, diagnostics });
                        }
                    }
                    (None, Some("$/lean/fileProgress")) => {
                        let params = &message["params"];
                        let path = params["textDocument"]["uri"].as_str().and_then(path_of);
                        let processing = params["processing"].as_array().into_iter().flatten();
                        let checking = processing.filter_map(|p| p["range"]["start"]["line"].as_u64()).min().map(|line| line as u32);
                        if let Some(path) = path {
                            let _ = events.unbounded_send(Event::Progress { path, checking });
                        }
                    }
                    _ => {}
                }
            }
            // The server is gone: what's pending fails (its senders drop), and nothing more is sent.
            alive.store(false, Ordering::SeqCst);
            pending.lock().unwrap().clear();
            queue.lock().unwrap().clear();
            // Give its stderr a moment to arrive.
            std::thread::sleep(std::time::Duration::from_millis(100));
            let lines = lines.lock().unwrap();
            let why = lines.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default();
            let _ = events.unbounded_send(Event::Exited { server: name, why });
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
                        "references": {},
                        "rename": { "prepareSupport": false },
                        "formatting": {},
                        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                        "signatureHelp": { "signatureInformation": { "parameterInformation": { "labelOffsetSupport": true } } },
                        "codeAction": {
                            "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] } },
                            "resolveSupport": { "properties": ["edit"] },
                            "dataSupport": true,
                        },
                        "publishDiagnostics": { "relatedInformation": false, "versionSupport": true },
                        "semanticTokens": {
                            "requests": { "full": { "delta": true } },
                            "tokenTypes": crate::convert::TOKEN_TYPES,
                            "tokenModifiers": ["declaration", "definition", "readonly", "static", "deprecated", "defaultLibrary"],
                            "formats": ["relative"],
                        },
                    },
                    "workspace": {
                        "configuration": true,
                        "workspaceFolders": true,
                        "semanticTokens": { "refreshSupport": true },
                        "applyEdit": true,
                        "workspaceEdit": { "documentChanges": true },
                    },
                    "window": { "showDocument": { "support": true } },
                },
            },
        });
        let _ = client.outbox.send(init);
        Ok(client)
    }

    /// Whether the server is still running.
    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// The last lines it wrote to stderr.
    pub fn stderr(&self) -> Vec<String> {
        self.stderr.lock().unwrap().iter().cloned().collect()
    }

    fn send(&self, message: Value) {
        if !self.alive() {
            return;
        }
        if self.ready.load(Ordering::SeqCst) {
            let _ = self.outbox.send(message);
            return;
        }
        let mut queue = self.queue.lock().unwrap();
        // It may have become ready while we waited for the lock.
        if self.ready.load(Ordering::SeqCst) {
            drop(queue);
            let _ = self.outbox.send(message);
        } else {
            queue.push(message);
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Ask the server; the answer is an error if it isn't running (or stops before answering).
    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Result<Value, String>> {
        self.request_cancelable(method, params).1
    }

    /// [`Client::request`], and its id, for [`Client::cancel`].
    pub fn request_cancelable(&self, method: &str, params: Value) -> (i64, oneshot::Receiver<Result<Value, String>>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        (id, self.send_request(id, method, params))
    }

    /// The answer to request `id` isn't wanted any more (`$/cancelRequest`):
    /// the server can stop working on it, and it fails.
    pub fn cancel(&self, id: i64) {
        if self.pending.lock().unwrap().remove(&id).is_some() {
            self.notify("$/cancelRequest", json!({ "id": id }));
        }
    }

    fn send_request(&self, id: i64, method: &str, params: Value) -> oneshot::Receiver<Result<Value, String>> {
        let (tx, rx) = oneshot::channel();
        if !self.alive() {
            let _ = tx.send(Err(format!("{} isn't running", self.name)));
            return rx;
        }
        self.pending.lock().unwrap().insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx
    }

    /// `workspace/executeCommand`.
    pub fn execute(&self, command: &str, arguments: Value) -> oneshot::Receiver<Result<Value, String>> {
        self.request("workspace/executeCommand", json!({ "command": command, "arguments": arguments }))
    }

    /// The names of the token types and modifiers its semantic tokens are
    /// numbered by, if it has semantic tokens for whole files.
    pub fn semantic_legend(&self) -> Option<(Vec<String>, Vec<String>)> {
        let caps = self.capabilities.lock().unwrap();
        let provider = &caps["semanticTokensProvider"];
        if provider["full"].is_null() || provider["full"] == false {
            return None;
        }
        let names = |key: &str| provider["legend"][key].as_array().map(|a| a.iter().filter_map(|n| Some(n.as_str()?.to_string())).collect());
        Some((names("tokenTypes")?, names("tokenModifiers").unwrap_or_default()))
    }

    /// Whether it does `provider` (`referencesProvider`, `renameProvider`, …), as it announced.
    pub fn supports(&self, provider: &str) -> bool {
        let caps = self.capabilities.lock().unwrap();
        !caps[provider].is_null() && caps[provider] != false
    }

    /// Characters that bring up the signature being typed.
    pub fn signature_triggers(&self) -> Vec<String> {
        let caps = self.capabilities.lock().unwrap();
        let provider = &caps["signatureHelpProvider"];
        let chars = |key: &str| provider[key].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect::<Vec<_>>();
        [chars("triggerCharacters"), chars("retriggerCharacters")].concat()
    }

    /// Whether it takes a file's changes as edits (else the whole text each time).
    pub fn incremental_sync(&self) -> bool {
        let caps = self.capabilities.lock().unwrap();
        let sync = &caps["textDocumentSync"];
        sync == 2 || sync["change"] == 2
    }

    /// Whether it can send only what changed in a file's semantic tokens.
    pub fn semantic_delta(&self) -> bool {
        self.capabilities.lock().unwrap()["semanticTokensProvider"]["full"]["delta"] == true
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
    /// Ask it to exit, then make sure it has, and reap it (off this thread).
    fn drop(&mut self) {
        let _ = self.outbox.send(json!({ "jsonrpc": "2.0", "method": "exit" }));
        let Some(mut child) = self.child.get_mut().ok().and_then(Option::take) else { return };
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

fn write(w: &mut impl std::io::Write, message: &Value) -> std::io::Result<()> {
    let body = message.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    w.flush()
}

enum Read {
    Message(Value),
    /// A message that isn't JSON even with its lone surrogates mended.
    Garbled,
    /// The stream ended (or broke).
    End,
}

fn read_message(reader: &mut impl std::io::BufRead) -> Read {
    let mut length = None;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => return Read::End,
            Ok(_) => {}
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(n) = header.strip_prefix("Content-Length:") {
            length = n.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length else { return Read::Garbled };
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return Read::End;
    }
    match serde_json::from_slice(&body) {
        Ok(message) => Read::Message(message),
        Err(_) => serde_json::from_slice(&mend_surrogates(&body)).map_or(Read::Garbled, Read::Message),
    }
}

/// `\uD83D` escapes without their other half (a JavaScript server cutting a
/// string inside an emoji), which JSON parsers refuse, as `�`.
fn mend_surrogates(body: &[u8]) -> Vec<u8> {
    let hex = |at: usize| body.get(at..at + 4).and_then(|h| u16::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
    let escape = |at: usize| (body.get(at..at + 2) == Some(b"\\u")).then(|| hex(at + 2)).flatten();
    let (mut out, mut i) = (Vec::with_capacity(body.len()), 0);
    while i < body.len() {
        if body[i] == b'\\' && body.get(i + 1) == Some(&b'\\') {
            out.extend_from_slice(b"\\\\");
            i += 2;
            continue;
        }
        if let Some(unit) = escape(i) {
            let high = (0xD800..0xDC00).contains(&unit);
            let low = (0xDC00..0xE000).contains(&unit);
            if high && escape(i + 6).is_some_and(|next| (0xDC00..0xE000).contains(&next)) {
                out.extend_from_slice(&body[i..i + 12]);
                i += 12;
                continue;
            }
            if high || low {
                out.extend_from_slice(b"\\uFFFD");
                i += 6;
                continue;
            }
        }
        out.push(body[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Read, read_message};

    fn next(input: &mut &[u8]) -> Option<serde_json::Value> {
        match read_message(input) {
            Read::Message(m) => Some(m),
            Read::Garbled => None,
            Read::End => panic!("ended"),
        }
    }

    #[test]
    fn reads_framed_messages() {
        let mut input: &[u8] = b"Content-Length: 13\r\nContent-Type: x\r\n\r\n{\"id\": 4.0}  Content-Length: 2\r\n\r\n{}";
        let first = next(&mut input).unwrap();
        assert_eq!(first["id"], 4.0);
    }

    #[test]
    fn a_garbled_message_is_mended_or_skipped_and_reading_goes_on() {
        let lone = br#"{"m":"x\ud83d y","pair":"\ud83d\ude00","esc":"\\ud83d"}"#;
        let bad = b"{not json";
        let mut input = Vec::new();
        for body in [&lone[..], bad, b"{}"] {
            input.extend(format!("Content-Length: {}\r\n\r\n", body.len()).bytes());
            input.extend(body);
        }
        let mut input = &input[..];
        let mended = next(&mut input).expect("mended");
        assert_eq!(mended["m"], "x\u{fffd} y");
        assert_eq!(mended["pair"], "\u{1f600}", "a whole pair is left be");
        assert_eq!(mended["esc"], "\\ud83d", "an escaped backslash isn't an escape");
        assert!(next(&mut input).is_none(), "skipped");
        assert_eq!(next(&mut input), Some(serde_json::json!({})), "and the next one's read");
        assert!(matches!(read_message(&mut input), Read::End));
    }
}
