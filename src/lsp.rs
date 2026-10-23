//! A small language-server client: one server per language per project,
//! spoken to over stdio with JSON-RPC. Documents are synced whole on every
//! change; requests are answered through oneshot channels; diagnostics
//! arrive as events.

use std::collections::HashMap;
use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};

/// A language server Ghostwrite knows how to start, and what it serves.
pub struct Spec {
    pub name: &'static str,
    command: &'static [&'static str],
    /// File extension → LSP language id.
    languages: &'static [(&'static str, &'static str)],
}

const SPECS: &[Spec] = &[
    Spec { name: "rust-analyzer", command: &["rust-analyzer"], languages: &[("rs", "rust")] },
    Spec { name: "basedpyright", command: &["basedpyright-langserver", "--stdio"], languages: &[("py", "python")] },
    Spec { name: "pyright", command: &["pyright-langserver", "--stdio"], languages: &[("py", "python")] },
    Spec { name: "pylsp", command: &["pylsp"], languages: &[("py", "python")] },
    Spec {
        name: "typescript-language-server",
        command: &["typescript-language-server", "--stdio"],
        languages: &[("ts", "typescript"), ("tsx", "typescriptreact"), ("js", "javascript"), ("jsx", "javascriptreact")],
    },
    Spec { name: "gopls", command: &["gopls"], languages: &[("go", "go")] },
    Spec {
        name: "clangd",
        command: &["clangd"],
        languages: &[("c", "c"), ("h", "c"), ("cc", "cpp"), ("cpp", "cpp"), ("hpp", "cpp"), ("cxx", "cpp")],
    },
];

/// Where each server's binary is, looked up once through the login shell (a
/// Finder-launched app has a minimal PATH).
fn installed() -> &'static HashMap<&'static str, PathBuf> {
    static FOUND: OnceLock<HashMap<&'static str, PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
        let script: String = SPECS.iter().map(|s| format!("command -v {} 2>/dev/null || echo; ", s.command[0])).collect();
        let out = Command::new(shell).args(["-lc", &script]).stderr(Stdio::null()).output();
        let lines: Vec<String> = out.map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()).unwrap_or_default();
        SPECS
            .iter()
            .zip(lines)
            .filter(|(_, path)| path.starts_with('/'))
            .map(|(spec, path)| (spec.name, PathBuf::from(path)))
            .collect()
    })
}

/// The project's Python environment: `$VIRTUAL_ENV`, or a `.venv` / `venv`
/// in the project or a folder above it.
pub fn python_env(root: &Path) -> Option<PathBuf> {
    let env = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).filter(|p| p.join("bin/python").exists());
    env.or_else(|| {
        root.ancestors()
            .flat_map(|dir| [dir.join(".venv"), dir.join("venv")])
            .find(|venv| venv.join("bin/python").exists())
    })
}

/// What `uv run` would use in `root`, if uv is installed.
fn uv_python(root: &Path) -> Option<PathBuf> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let out = Command::new(shell).args(["-lc", "uv python find 2>/dev/null"]).current_dir(root).stderr(Stdio::null()).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    path.starts_with('/').then(|| PathBuf::from(path))
}

/// The Python to analyze with: what `uv run` uses, else the project's
/// environment, else the login shell's `python3` (pyright looks for
/// `python`, which macOS doesn't have).
pub fn python(root: &Path) -> Option<PathBuf> {
    static SYSTEM: OnceLock<Option<PathBuf>> = OnceLock::new();
    static UV: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    let uv = UV.get_or_init(Mutex::default).lock().unwrap().entry(root.to_path_buf()).or_insert_with(|| uv_python(root)).clone();
    uv.or_else(|| python_env(root).map(|venv| venv.join("bin/python"))).or_else(|| {
        SYSTEM
            .get_or_init(|| {
                let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
                let out = Command::new(shell).args(["-lc", "command -v python3"]).stderr(Stdio::null()).output().ok()?;
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                path.starts_with('/').then(|| PathBuf::from(path))
            })
            .clone()
    })
}

/// Whether the project configures pyright / basedpyright itself.
fn has_pyright_config(root: &Path) -> bool {
    root.join("pyrightconfig.json").exists()
        || std::fs::read_to_string(root.join("pyproject.toml"))
            .is_ok_and(|t| t.contains("[tool.pyright]") || t.contains("[tool.basedpyright]"))
}

/// Answers to `workspace/configuration`, by section: the project's Python,
/// and pyright's "standard" checking unless the project chose its own.
fn settings(section: &str, root: &Path) -> Value {
    let python = python(root);
    let checking = (!has_pyright_config(root)).then_some("standard");
    match section {
        "python" => json!({ "pythonPath": python }),
        "python.analysis" | "basedpyright.analysis" => json!({ "typeCheckingMode": checking }),
        "basedpyright" | "pyright" => json!({ "analysis": { "typeCheckingMode": checking } }),
        _ => Value::Null,
    }
}

/// The installed server for a file, and the file's language id.
pub fn spec_for(path: &Path) -> Option<(&'static Spec, &'static str)> {
    let ext = path.extension()?.to_str()?;
    SPECS.iter().find_map(|spec| {
        let language = spec.languages.iter().find(|(e, _)| *e == ext)?.1;
        installed().contains_key(spec.name).then_some((spec, language))
    })
}

pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// Byte offset → LSP position (line, UTF-16 column).
pub fn position(text: &str, offset: usize) -> lsp_types::Position {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..line_start].matches('\n').count();
    let character: usize = text[line_start..offset].chars().map(char::len_utf16).sum();
    lsp_types::Position::new(line as u32, character as u32)
}

/// LSP position → byte offset, clamped to the line.
pub fn offset(text: &str, pos: lsp_types::Position) -> usize {
    let line_start = if pos.line == 0 {
        0
    } else {
        text.match_indices('\n').nth(pos.line as usize - 1).map_or(text.len(), |(i, _)| i + 1)
    };
    let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
    let mut units = 0;
    for (i, c) in text[line_start..line_end].char_indices() {
        if units >= pos.character as usize {
            return line_start + i;
        }
        units += c.len_utf16();
    }
    line_end
}

pub enum Event {
    Diagnostics { path: PathBuf, diagnostics: Vec<lsp_types::Diagnostic> },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    pub name: &'static str,
    writer: Arc<Mutex<ChildStdin>>,
    next_id: AtomicI64,
    pending: Pending,
    /// Messages wait here until the server has answered `initialize`.
    ready: Arc<AtomicBool>,
    queue: Arc<Mutex<Vec<Value>>>,
    pub capabilities: Arc<Mutex<Value>>,
    child: Mutex<Child>,
}

fn write(writer: &Mutex<ChildStdin>, message: &Value) {
    let body = message.to_string();
    if let Ok(mut w) = writer.lock() {
        let _ = write!(w, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = w.flush();
    }
}

impl Client {
    pub fn start(spec: &'static Spec, root: &Path, events: mpsc::UnboundedSender<Event>) -> std::io::Result<Arc<Client>> {
        let binary = installed().get(spec.name).cloned().unwrap_or_else(|| PathBuf::from(spec.command[0]));
        let mut command = Command::new(binary);
        // Servers that look for the active environment find the project's.
        let venv = python(root).and_then(|p| p.parent()?.parent().map(Path::to_path_buf)).filter(|v| v.join("pyvenv.cfg").exists());
        if let Some(venv) = venv.or_else(|| python_env(root)) {
            let path = std::env::var("PATH").unwrap_or_default();
            command.env("VIRTUAL_ENV", &venv).env("PATH", format!("{}:{path}", venv.join("bin").display()));
        }
        let mut child = command
            .args(&spec.command[1..])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let client = Arc::new(Client {
            name: spec.name,
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
        let project = root.to_path_buf();
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
                        ready.store(true, Ordering::SeqCst);
                        for queued in queue.lock().unwrap().drain(..) {
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
                    // Requests from the server: answer so it never waits on us.
                    (Some(_), Some(method)) => {
                        let result = match method {
                            "workspace/configuration" => Value::Array(
                                message["params"]["items"]
                                    .as_array()
                                    .map(|items| items.iter().map(|i| settings(i["section"].as_str().unwrap_or(""), &project)).collect())
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
        });

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
            write(&self.writer, &message);
        } else {
            let mut queue = self.queue.lock().unwrap();
            // It may have become ready while we waited for the lock.
            if self.ready.load(Ordering::SeqCst) {
                drop(queue);
                write(&self.writer, &message);
            } else {
                queue.push(message);
            }
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

    /// Characters that pop the completion menu, as the server announced them.
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

/// The servers of one project, started on first use.
pub struct Servers {
    root: PathBuf,
    clients: HashMap<&'static str, Option<Arc<Client>>>,
    events: mpsc::UnboundedSender<Event>,
    /// Off in tests, which open .rs and .py files without wanting servers.
    pub enabled: bool,
}

impl Servers {
    pub fn new(root: PathBuf, events: mpsc::UnboundedSender<Event>) -> Self {
        Self { root, clients: HashMap::new(), events, enabled: !cfg!(test) }
    }

    /// The server for `path` (starting it if needed), and the file's language id.
    pub fn for_path(&mut self, path: &Path) -> Option<(Arc<Client>, &'static str)> {
        if !self.enabled {
            return None;
        }
        let (spec, language) = spec_for(path)?;
        let client = self
            .clients
            .entry(spec.name)
            .or_insert_with(|| {
                Client::start(spec, &self.root, self.events.clone())
                    .inspect_err(|e| eprintln!("couldn't start {}: {e}", spec.name))
                    .ok()
            })
            .clone()?;
        Some((client, language))
    }
}

#[cfg(test)]
mod tests {
    use super::{offset, path_of, position, uri};

    #[test]
    fn positions_count_utf16() {
        let text = "ab\nä😀x\n";
        // 'x' is after ä (1 unit) and 😀 (2 units).
        let x = text.find('x').unwrap();
        assert_eq!(position(text, x), lsp_types::Position::new(1, 3));
        assert_eq!(offset(text, lsp_types::Position::new(1, 3)), x);
        assert_eq!(offset(text, lsp_types::Position::new(1, 99)), text.len() - 1, "clamped to the line");
        assert_eq!(offset(text, lsp_types::Position::new(0, 1)), 1);
    }

    #[test]
    fn uris_round_trip() {
        let p = std::path::Path::new("/tmp/my dir/a#b.rs");
        assert_eq!(uri(p), "file:///tmp/my%20dir/a%23b.rs");
        assert_eq!(path_of(&uri(p)).unwrap(), p);
    }

    #[test]
    fn python_settings_follow_the_project() {
        use super::{python_env, settings};
        let root = std::env::temp_dir().join(format!("ghostwrite-py-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/.venv/bin")).unwrap();
        std::fs::write(root.join("app/.venv/bin/python"), "").unwrap();
        let app = root.join("app");
        assert_eq!(python_env(&app.join("pkg")), Some(app.join(".venv")), "found from a folder below");
        // uv is asked first (and ignores this fake environment); some Python is always given.
        assert!(settings("python", &app)["pythonPath"].is_string());
        assert_eq!(settings("basedpyright.analysis", &app)["typeCheckingMode"], "standard");
        std::fs::write(app.join("pyproject.toml"), "[tool.basedpyright]\ntypeCheckingMode = \"strict\"\n").unwrap();
        assert!(settings("basedpyright.analysis", &app)["typeCheckingMode"].is_null(), "the project's own choice wins");
        std::fs::remove_dir_all(root).ok();
    }

    /// Live: in a uv project, the Python `uv run` uses. `cargo test uv_python_matches -- --ignored`
    #[test]
    #[ignore]
    fn uv_python_matches() {
        let mulusk = std::path::Path::new("/Users/frisbro/Documents/Projects/adventures/mulusk");
        println!("{:?}", super::python(mulusk));
        assert!(super::python(mulusk).is_some_and(|p| p.ends_with(".venv/bin/python3") || p.ends_with(".venv/bin/python")));
    }
}
