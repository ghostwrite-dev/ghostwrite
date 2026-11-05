//! A stand-in for a web server, for tests: it answers each path with what it's been
//! given, and anything else with 404. What it serves can change as it runs.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::sync::{Arc, Mutex};

/// What it serves, by path: a status and a body.
type Served = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

#[doc(hidden)]
pub struct StandIn {
    /// Where it is: `http://127.0.0.1:‹port›`.
    pub url: String,
    served: Served,
    asked: Arc<Mutex<Vec<String>>>,
}

impl StandIn {
    pub fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (served, asked): (Served, Arc<Mutex<Vec<String>>>) = Default::default();
        let (serving, asking) = (served.clone(), asked.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(&stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut headers = vec![];
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).map_or(true, |n| n == 0) || line.trim().is_empty() {
                        break;
                    }
                    headers.push(line.trim().to_string());
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                asking.lock().unwrap().push(format!("{path} {}", headers.join(" | ")));
                let (status, body) = serving.lock().unwrap().get(&path).cloned().unwrap_or((404, b"{\"message\": \"Not Found\"}".to_vec()));
                let mut stream = &stream;
                let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
        });
        Self { url, served, asked }
    }

    /// Answer `path` with `body`.
    pub fn serve(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.answer(path, 200, body);
    }

    pub fn answer(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.served.lock().unwrap().insert(path.to_string(), (status, body.into()));
    }

    /// The paths it's been asked for, each with the headers it was asked with.
    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}
