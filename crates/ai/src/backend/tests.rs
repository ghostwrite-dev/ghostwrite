//! What the backends' tests share. They check the harness against a canned
//! model and a stand-in API; and live, against Claude Code and a local model:
//! `cargo test -p ghostwrite-ai -- --ignored`.

use super::Listener;
use crate::context::Request;

pub(super) const DONE: &str = "<done>\n</done>\n";

#[derive(Default)]
pub(super) struct Collect(pub(super) String, pub(super) Vec<String>, pub(super) Option<String>);

impl Listener for Collect {
    fn text(&mut self, chunk: &str) {
        self.0.push_str(chunk);
    }
    fn looking(&mut self, what: String) {
        self.1.push(what);
    }
    fn session(&mut self, id: String) {
        self.2 = Some(id);
    }
}

/// A project with a picture and a PDF in it, and a request about it.
pub(super) fn with_files() -> (std::path::PathBuf, Request) {
    let root = util_project("api-files");
    let mut picture = std::io::Cursor::new(vec![]);
    image::RgbImage::new(2, 2).write_to(&mut picture, image::ImageFormat::Png).unwrap();
    std::fs::write(root.join("figure.png"), picture.into_inner()).unwrap();
    std::fs::write(root.join("paper.pdf"), b"%PDF-1.4\nthe paper").unwrap();
    let req = asked_in(&root);
    (root, req)
}

pub(super) fn asked() -> Request {
    Request {
        path: None,
        language: "rust".into(),
        range: 0..0,
        text: "fn main() {}\n".into(),
        instruction: "what does this do?".into(),
        followups: vec![],
        root: None,
        others: vec![],
        surroundings: Default::default(),
        session: None,
        fork: false,
        small: 0,
    }
}

pub(super) fn asked_in(root: &std::path::Path) -> Request {
    Request { root: Some(root.to_path_buf()), ..asked() }
}

pub(super) fn util_project(name: &str) -> std::path::PathBuf {
    let root = ghostwrite_project::scratch_dir(name);
    std::fs::write(root.join("util.py"), "def load(): pass\n").unwrap();
    root
}

/// The file that answers it isn't shown: it has to be looked at, with the tools.
pub(super) fn asked_about_another_file(root: &std::path::Path, text: &str) -> Request {
    Request {
        path: Some(root.join("main.py")),
        language: "python".into(),
        range: 0..text.len(),
        text: text.into(),
        instruction: "what does load return when the file is missing? read util.py to find out".into(),
        root: Some(root.to_path_buf()),
        ..asked()
    }
}

/// A small project: main.py calls into util.py, which can fail.
pub(super) fn two_file_project() -> (std::path::PathBuf, String) {
    let root = ghostwrite_project::scratch_dir("live");
    let main = "from util import load\n\n\ndef main():\n    data = load(\"config.json\")\n    print(data[\"name\"])\n\n\nmain()\n";
    std::fs::write(root.join("main.py"), main).unwrap();
    let util = "import json\n\n\ndef load(path):\n    try:\n        with open(path) as f:\n            return json.load(f)\n    except FileNotFoundError:\n        return {\"name\": \"default\"}\n";
    std::fs::write(root.join("util.py"), util).unwrap();
    (root, main.to_string())
}
