//! What language servers and plugins add: moving a file to another server, Lean's goals, formatting and renaming, a PDF, a Typst file's pages. Most need the server installed, and are ignored otherwise.

use super::*;
use ghostwrite_core::Mode;

/// Live: needs `tinymist` installed.
#[gpui::test]
#[ignore]
fn a_renamed_file_moves_to_its_new_languages_server(cx: &mut TestAppContext) {
    let mut t = open_with("= Title\n", &[], true, cx);
    let server = |t: &mut Setup| t.read(|e| e.tab().unwrap().doc.lsp.as_ref().map(|l| l.server()));
    assert_eq!(server(&mut t), None, "plain text has no server");
    t.command("w paper.typ");
    assert_eq!(t.active().as_deref(), Some("paper.typ"));
    assert_eq!(server(&mut t), Some("tinymist"), "writing it as .typ starts tinymist");
    t.keys("space e r").keys("cmd-a").input("paper.md").keys("enter");
    assert_eq!(t.active().as_deref(), Some("paper.md"));
    assert_eq!(server(&mut t), None, "renaming it to .md leaves tinymist");
}

/// Live: needs `lake` (elan) installed.
#[gpui::test]
#[ignore]
fn a_lean_file_shows_the_goals_at_the_cursor(cx: &mut TestAppContext) {
    let mut t = open_with("", &[], true, cx);
    let text = "theorem t (n : Nat) : n + 0 = n := by\n  rfl\n\n#eval 6 * 7\n";
    let path = t.dir.join("Main.lean");
    std::fs::write(&path, text).unwrap();
    t.editor.update(t.cx, |e, cx| {
        e.project.aside_shown = true;
        e.open_paths(vec![path], cx);
    });
    let wait_for = |t: &mut Setup, what: &dyn Fn(&Editor) -> bool| {
        for _ in 0..600 {
            t.cx.run_until_parked();
            if t.read(what) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            t.cx.update(|window, _| window.refresh());
        }
        panic!("timed out");
    };
    let goals = |e: &Editor| {
        let blocks = e.tab().and_then(|t| Some(t.doc.aside.as_ref()?.panel.blocks()))?;
        // Before there's an answer it only says there are none.
        (!matches!(blocks.as_slice(), [ghostwrite_plugin::Block::Muted(_)])).then_some(blocks)
    };
    t.keys("j w");
    wait_for(&mut t, &|e| goals(e).is_some());
    assert!(
        t.read(goals).is_some_and(|blocks| blocks.iter().any(|b| matches!(b, ghostwrite_plugin::Block::Code { text, .. } if text == "n : Nat\n⊢ n + 0 = n"))),
        "the goal at the cursor"
    );
    t.keys("j j");
    wait_for(&mut t, &|e| e.tab().is_some_and(|t| t.doc.diagnostics.iter().any(|d| d.message == "42")));
}

/// Live: needs `rust-analyzer` installed.
#[gpui::test]
#[ignore]
fn the_language_server_formats_renames_and_lists_symbols(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open_with("", &[], true, cx);
    std::fs::write(t.dir.join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/main.rs"), "fn add(a: i32, b: i32) -> i32 {   a + b }\nfn main() { let x = add(1, 2); println!(\"{x}\"); }\n").unwrap();
    t.command("e src/main.rs");
    let wait = |t: &mut Setup, what: &str, done: &dyn Fn(&Editor) -> bool, again: &dyn Fn(&mut Setup)| {
        for round in 0..300 {
            t.cx.run_until_parked();
            if t.read(done) {
                return;
            }
            if round % 25 == 0 {
                again(t);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("timed out waiting for {what}: {}", t.text());
    };
    wait(&mut t, "formatting", &|e| e.tab().unwrap().doc.buffer.to_string().contains("i32 {\n    a + b\n}"), &|t| {
        t.command("format");
    });
    wait(&mut t, "renaming", &|e| e.tab().unwrap().doc.buffer.to_string().matches("plus").count() == 2, &|t| {
        t.keys("escape g g f d space r");
        assert_eq!(
            t.read(|e| e.prompt.as_ref().map(|p| p.field.clone())).map(|f| t.cx.update(|_, cx| f.read(cx).text().to_string())).as_deref(),
            Some("add"),
            "the prompt starts with its name"
        );
        t.keys("cmd-a").input("plus").keys("enter");
    });
    wait(&mut t, "symbols", &|e| e.picker.is_some(), &|t| {
        t.keys("escape space s");
    });
    t.input("main").keys("enter");
    assert_eq!(t.read(|e| e.tab().map(|t| t.doc.buffer.line_of(t.cursor()))), Some(3), "main, picked");
    t.keys("g g f s");
    wait(&mut t, "references", &|e| e.story.is_some(), &|t| {
        t.keys("g r");
    });
    assert!(t.status().is_none_or(|s| !s.contains("no references")));
}

/// A PDF of two pages, the first half filled in.
fn two_page_pdf() -> Vec<u8> {
    let content = "0 0 0 rg 0 0 100 50 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 5 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut bytes = String::from("%PDF-1.4\n");
    let mut offsets = vec![];
    for (ix, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.push_str(&format!("{} 0 obj\n{object}\nendobj\n", ix + 1));
    }
    let xref = bytes.len();
    bytes.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for offset in offsets {
        bytes.push_str(&format!("{offset:010} 00000 n \n"));
    }
    bytes.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1));
    bytes.into_bytes()
}

/// Until the document in view has pages, or it's said why it can't.
fn until_it_has_pages(t: &mut Setup) {
    for _ in 0..3000 {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
        let done = t.read(|e| {
            let tab = e.tab().unwrap();
            let rendered = tab.doc.rendered.as_ref();
            rendered.is_some_and(|rendered| rendered.error.is_some() || (rendered.pages.is_some() && !rendered.is_making()))
        });
        if done {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("its pages never came");
}

#[gpui::test]
fn a_pdf_is_its_pages_read_only_and_the_model_leaves_it_be(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &["unused"], cx);
    let pdf = t.dir.join("paper.pdf");
    let original = two_page_pdf();
    std::fs::write(&pdf, &original).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![pdf.clone()], cx)));
    until_it_has_pages(&mut t);
    assert_eq!(t.active().as_deref(), Some("paper.pdf"));
    assert_eq!(t.text(), "── page 1 ──\n── page 2 ──", "a line for each page, to move by");
    let pages = t.read(|e| e.tab().unwrap().doc.rendered.as_ref().unwrap().pages.clone().unwrap());
    assert_eq!(pages.sizes().len(), 2);
    let drawn = pages.draw_at(0, 400).expect("it's drawn");
    let pixel = |x: usize, y: usize| drawn.bgra[(y * drawn.width + x) * 4..][..4].to_vec();
    assert_eq!((drawn.width, drawn.height), (400, 200));
    assert_eq!(pixel(50, 150), [0, 0, 0, 255], "what's on it, the right way up");
    assert_eq!(pixel(350, 50), [255, 255, 255, 255], "on a white page");

    t.keys("j");
    assert_eq!(t.read(|e| e.tab().unwrap().cursor_position().0), 1, "j goes to the next page");
    assert_eq!(pages.cursor(t.cursor()).map(|place| place.page), Some(1));
    t.keys("x d");
    assert_eq!(t.text(), "── page 1 ──\n── page 2 ──", "an edit is undone");
    assert_eq!(t.status().as_deref(), Some("this is read-only"));
    t.keys("i");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "no inserting");
    t.command("w");
    assert_eq!(std::fs::read(&pdf).unwrap(), original, "and it's never written");

    t.keys("space a");
    assert!(!t.reviewing(), "the model doesn't comment on it");
    assert!(t.status().is_some_and(|s| s.contains("doesn't comment on this")), "{:?}", t.status());

    // Rebuilt: its pages are read again, and it's still not something to save.
    let key = pages.key(0);
    let rebuilt = String::from_utf8(original.clone()).unwrap().replace("0 0 100 50 re", "0 0 150 50 re");
    std::fs::write(&pdf, rebuilt).unwrap();
    let disk = t.read(|e| e.project.to_reread(std::slice::from_ref(&pdf))).read();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| {
        editor.update(cx, |e, _| {
            e.project.reread(disk);
        })
    });
    for _ in 0..3000 {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
        if t.read(|e| e.tab().unwrap().doc.rendered.as_ref().unwrap().pages.as_ref().is_some_and(|pages| pages.key(0) != key)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let now = t.read(|e| e.tab().unwrap().doc.rendered.as_ref().unwrap().pages.clone().unwrap());
    assert_ne!(now.key(0), key, "a rebuilt file is shown as it is now");
    assert!(!t.read(|e| e.tab().unwrap().doc.dirty));
}

#[gpui::test]
fn an_image_opens_read_only_at_its_own_size_and_the_model_leaves_it_be(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &["unused"], cx);
    let file = t.dir.join("icon.bmp");
    // A BMP of one red pixel: 24 bits, bottom up, each row padded to four bytes.
    let mut bytes = b"BM".to_vec();
    bytes.extend(58u32.to_le_bytes());
    bytes.extend([0; 4]);
    bytes.extend(54u32.to_le_bytes());
    bytes.extend(40u32.to_le_bytes());
    bytes.extend(1i32.to_le_bytes());
    bytes.extend(1i32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(24u16.to_le_bytes());
    bytes.extend([0; 24]);
    bytes.extend([0, 0, 255, 0]);
    std::fs::write(&file, &bytes).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![file.clone()], cx)));
    until_it_has_pages(&mut t);
    let pages = t.read(|e| e.tab().unwrap().doc.rendered.as_ref().unwrap().pages.clone().unwrap());
    assert_eq!(pages.max_width(), Some(1.), "a tiny image isn't blown up to the width of the window");
    let drawn = pages.draw_at(0, 1).expect("it's drawn");
    assert_eq!(drawn.bgra, [0, 0, 255, 255], "red, as blue, green, red and alpha");
    t.keys("x d");
    assert_eq!(t.status().as_deref(), Some("this is read-only"));
    t.command("w");
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "and it's never written");
    t.keys("space a");
    assert!(!t.reviewing(), "the model doesn't comment on it");
}

#[gpui::test]
fn a_pdf_that_isnt_one_says_so_and_nothing_goes_wrong(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    let pdf = t.dir.join("broken.pdf");
    std::fs::write(&pdf, "this is no PDF").unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![pdf.clone()], cx)));
    until_it_has_pages(&mut t);
    assert!(t.status().is_some_and(|said| said.contains("couldn't read broken.pdf as a PDF")), "{:?}", t.status());
    assert_eq!(std::fs::read(&pdf).unwrap(), b"this is no PDF");
    t.keys("i");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "it's read-only, even with no pages");
}

/// A click on a real page: the view stays, and the file has the cursor there for going back.
#[gpui::test]
fn a_double_click_on_a_page_goes_to_the_file(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let path = t.dir.join("paper.typ");
    let source = "#set page(width: 300pt, height: 200pt)\nThe cat sat on the mat, and the mat was glad of it, all day long.\n";
    std::fs::write(&path, source).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![path.clone()], cx)));
    let show = |t: &mut Setup| {
        t.command("preview");
        let editor = t.editor.clone();
        t.cx.update(|_, cx| editor.update(cx, |e, cx| e.refresh_aside(cx)));
        t.cx.run_until_parked();
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
    };
    let double = |t: &mut Setup, at: gpui::Point<gpui::Pixels>| {
        for click_count in 1..=2 {
            let (button, modifiers) = (gpui::MouseButton::Left, gpui::Modifiers::none());
            t.cx.simulate_event(gpui::MouseDownEvent { button, position: at, modifiers, click_count, first_mouse: false });
            t.cx.simulate_event(gpui::MouseUpEvent { button, position: at, modifiers, click_count });
        }
        t.cx.run_until_parked();
    };
    let name = |t: &mut Setup| t.read(|e| e.tab().map(|tab| tab.doc.name().to_string()));
    show(&mut t);
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"));

    let place = t.read(|e| {
        let file = e.project.tabs.iter().find(|tab| tab.doc.path.as_deref() == Some(&path)).unwrap();
        file.doc.rendered.as_ref().unwrap().pages.as_ref().unwrap().cursor(source.find("cat").unwrap()).unwrap()
    });
    let bounds = t.cx.debug_bounds("page-view").expect("the pages are drawn");
    let scale = f32::from(bounds.size.width) / 300.;
    let at = gpui::point(bounds.origin.x + gpui::px((place.x + place.width / 2.) * scale), bounds.origin.y + gpui::px((place.y + place.height / 2.) * scale));
    double(&mut t, at);
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"), "to the file's tab");
    let cursor = t.read(|e| e.tab().unwrap().cursor());
    assert!(source[cursor..].starts_with("cat") || source[cursor..].starts_with("at"), "and to what was clicked: {:?}", &source[cursor..]);

    show(&mut t);
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"));
    let corner = gpui::point(bounds.origin.x + gpui::px(2.), bounds.origin.y + gpui::px(2.));
    double(&mut t, corner);
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"), "from a margin too");
    assert_eq!(t.read(|e| e.tab().unwrap().cursor()), cursor, "where the cursor already was");
}

#[gpui::test]
fn clicking_a_page_stays_on_it_and_the_file_is_there_when_you_go_back(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let path = t.dir.join("paper.typ");
    let source = "#set page(width: 300pt, height: 200pt)\nThe cat sat on the mat, and the mat was glad of it, all day long.\n";
    std::fs::write(&path, source).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![path.clone()], cx)));
    t.command("preview");
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.refresh_aside(cx)));
    t.cx.run_until_parked();
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    let name = |t: &mut Setup| t.read(|e| e.tab().map(|tab| tab.doc.name().to_string()));
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"));

    // Where a word of the text is drawn: the page fills the view's width, at the top.
    let window = t.cx.update(|window, _| window.bounds().size);
    let place = t.read(|e| {
        let file = e.project.tabs.iter().find(|tab| tab.doc.path.as_deref() == Some(&path)).unwrap();
        file.doc.rendered.as_ref().unwrap().pages.as_ref().unwrap().cursor(source.find("cat").unwrap()).unwrap()
    });
    assert!(place.y > 0.);
    let bounds = t.cx.debug_bounds("page-view").expect("the pages are drawn");
    let scale = f32::from(bounds.size.width) / 300.;
    let at = gpui::point(bounds.origin.x + gpui::px((place.x + place.width / 2.) * scale), bounds.origin.y + gpui::px((place.y + place.height / 2.) * scale));
    assert!(f32::from(at.y) < f32::from(window.height), "{at:?}");
    t.cx.simulate_click(at, gpui::Modifiers::none());
    t.cx.run_until_parked();

    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"), "it stays on the pages");
    t.command("preview");
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"));
    let cursor = t.read(|e| e.tab().unwrap().cursor());
    assert!(source[cursor..].starts_with("cat") || source[cursor..].starts_with("at"), "the cursor is on what was clicked: {:?}", &source[cursor..]);
}

#[gpui::test]
fn a_preview_beside_its_file_keeps_up_and_clicking_it_keeps_the_keys(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let path = t.dir.join("paper.typ");
    let source = "#set page(width: 300pt, height: 200pt)\nThe cat sat on the mat, and the mat was glad of it, all day long.\n";
    std::fs::write(&path, source).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![path.clone()], cx)));
    t.cx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(800.)));
    t.command("preview");
    t.command("preview");
    t.command("vsplit");
    let settle = |t: &mut Setup| {
        for _ in 0..4 {
            t.cx.update(|window, _| window.refresh());
            t.cx.run_until_parked();
        }
    };
    settle(&mut t);
    let name = |t: &mut Setup| t.read(|e| e.tab().map(|tab| tab.doc.name().to_string()));
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"), "the keys are on the file");
    let pages = t.read(|e| e.other_pane_doc()).expect("its pages are beside it");
    assert!(t.read(|e| e.pages_of(pages).is_some()));
    let made = |t: &mut Setup| t.read(|e| e.tab().unwrap().doc.rendered.as_ref().and_then(|r| r.made));
    let first = made(&mut t);
    assert!(first.is_some(), "they're made");
    t.keys("g g i").input("// a comment\n").keys("escape");
    settle(&mut t);
    assert_ne!(made(&mut t), first, "and made again as the file changes");

    let place = t.read(|e| {
        e.tab().unwrap().doc.rendered.as_ref().unwrap().pages.as_ref().unwrap().cursor(e.tab().unwrap().doc.buffer.to_string().find("cat").unwrap()).unwrap()
    });
    let bounds = t.cx.debug_bounds("other-pane").expect("drawn");
    let scale = f32::from(bounds.size.width) / 300.;
    let at = gpui::point(bounds.origin.x + gpui::px((place.x + place.width / 2.) * scale), bounds.origin.y + gpui::px((place.y + place.height / 2.) * scale));
    t.cx.simulate_click(at, gpui::Modifiers::none());
    t.cx.run_until_parked();
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"), "the keys stay on the file");
    let (text, cursor) = t.read(|e| (e.tab().unwrap().doc.buffer.to_string(), e.tab().unwrap().cursor()));
    // Within a character or so: the pane's edge moves the pages a little from where this works it out.
    assert!(
        text[cursor..].trim_start().starts_with("cat") || text[cursor..].starts_with("at"),
        "and the cursor goes to what was clicked: {:?}",
        &text[cursor..]
    );
}

/// `space v` is the key of the plugin's view of pages: to the pages of a file whose plugin
/// makes them, opening them, and back. A file with none says so.
#[gpui::test]
fn space_v_goes_to_a_files_pages_and_back(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let path = t.dir.join("paper.typ");
    std::fs::write(&path, "= Intro\n").unwrap();
    let editor = t.editor.clone();
    let name = |t: &mut Setup| t.read(|e| e.tab().map(|tab| tab.doc.name().to_string()));
    let tabs = t.read(|e| e.project.tabs.len());
    t.keys("space v");
    assert_eq!(t.read(|e| e.project.tabs.len()), tabs, "doc.txt has no pages");
    assert!(t.status().is_some_and(|said| said.starts_with("this file has no pages view: ") && said.contains("typst")), "{:?}", t.status());
    assert_eq!((t.at(), t.read(|e| e.modal.pending_keys())), ((ViewKind::Code, Mode::Normal), String::new()), "and the keys are done with");
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![path.clone()], cx)));
    t.keys("space v");
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"), "opened, in a tab of their own");
    let pages = |t: &mut Setup| matches!(t.at(), (ViewKind::Plugin(id), Mode::Normal) if id.plugin == "typst" && id.name == "pages");
    assert!(pages(&mut t), "the keys on them, its plugin's view: {:?}", t.at());
    assert_eq!(t.read(|e| e.project.tabs.len()), tabs + 2);
    t.keys("space v");
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"), "and back, from the view");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal));
    t.keys("space v");
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"), "again, to the same tab");
    assert_eq!(t.read(|e| e.project.tabs.len()), tabs + 2);
    t.keys("space v space e space v");
    assert!(pages(&mut t), "from Files, to the pages");
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ · pages"));
    t.keys("space v i");
    t.keys("space");
    t.input("v");
    t.keys("escape");
    assert_eq!(name(&mut t).as_deref(), Some("paper.typ"), "typing isn't a key binding");
    // The help lists the view's keys, drawn and all.
    t.keys("space ?");
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    assert!(t.read(|e| e.cheatsheet));
    assert!(t.read(|e| crate::editor::view::cheatsheet(e.modal.keymap()).iter().any(|(title, _)| title == "Pages  space v")));
    t.keys("escape");
}

/// No plugin takes a key that's built in, in any language.
#[gpui::test]
fn plugins_keys_are_free(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let cheatsheet = crate::editor::view::cheatsheet(t.read(|e| e.modal.keymap()));
    let builtin: Vec<(ghostwrite_core::ViewKind, &str)> =
        cheatsheet.iter().flat_map(|(_, rows)| rows.iter().map(|row| (row.view, row.keys.as_str()))).collect();
    let mut seen = 0;
    for language in ghostwrite_core::Language::all().into_iter().map(Some).chain([None]) {
        for binding in ghostwrite_plugin::keys(language) {
            seen += 1;
            // The help writes `space v`, but `gv`.
            let joined = binding.keys.replace(' ', "");
            let taken = |keys: &&str| keys.contains(binding.keys) || keys.split_whitespace().any(|key| key == joined);
            // `space v` to `:preview` was a plugin's own before its views had keys: the
            // view's comes first, and does what it did.
            if binding.keys == "space v" && binding.command == "preview" {
                continue;
            }
            // In its view, the code's keys are under that view's own, and both come first.
            // A plugin's views of a name share their keys.
            let there = builtin.iter().filter(|(view, _)| view.name() == binding.view.name() || *view == ghostwrite_core::ViewKind::Code).map(|(_, keys)| keys);
            assert!(!there.into_iter().any(taken), "{} is built in", binding.keys);
        }
    }
    // Nor do their views' keys: the editor's would come first.
    for view in ghostwrite_plugin::views() {
        let Some(key) = view.key else { continue };
        seen += 1;
        let editors = crate::editor::view::cheatsheet(ghostwrite_core::Keymap::standard());
        assert!(
            !editors.iter().flat_map(|(_, rows)| rows).any(|row| row.view == ghostwrite_core::ViewKind::Code && row.keys.contains(key)),
            "{key} is built in"
        );
    }
    assert!(t.read(|e| e.modal.keymap().clashes().is_empty()), "and they don't take each other's");
    assert!(seen >= 2, "the plugins bind some");
}

#[gpui::test]
fn a_typst_files_pages_are_a_tab_and_follow_it(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let source = "#set page(width: 300pt, height: 200pt)\n= Intro\nThe cat sat on the mat.\n";
    let path = t.dir.join("paper.typ");
    std::fs::write(&path, source).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.project.aside_shown = true;
            e.open_paths(vec![path.clone()], cx);
        })
    });
    let compiled = |t: &mut Setup| {
        t.cx.update(|_, cx| editor.update(cx, |e, cx| e.refresh_aside(cx)));
        t.cx.run_until_parked();
        t.read(|e| {
            let tab = e.tab().unwrap();
            let p = tab.doc.rendered.as_ref().unwrap();
            (p.pages.as_ref().map(|d| d.sizes().len()), p.error.clone(), tab.view.pages.line)
        })
    };
    let tabs = t.read(|e| e.project.tabs.len());
    assert!(t.read(|e| e.project.tabs.iter().all(|tab| tab.doc.view_of.is_none())), "no pages until they're asked for");
    t.command("preview");
    assert_eq!(t.read(|e| e.tab().map(|tab| tab.doc.name().to_string())).as_deref(), Some("paper.typ · pages"));
    t.keys("i");
    assert_eq!(t.status().as_deref(), Some("this is read-only"));
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "no inserting");
    t.keys("escape");
    t.command("preview");
    assert_eq!(t.active().as_deref(), Some("paper.typ"), ":preview goes back");
    assert_eq!(t.read(|e| e.project.tabs.len()), tabs + 1);
    assert_eq!(compiled(&mut t), (Some(1), None, None), "its page, as it opens, left where it was");
    let cat = source.find("cat").unwrap();
    let on_it = t.read(|e| e.tab().unwrap().doc.rendered.as_ref().unwrap().pages.as_ref().unwrap().cursor(cat));
    assert!(on_it.is_some(), "the c of cat is on the page, from the text");

    t.keys("j j A").input(" Again.").keys("escape");
    assert_eq!(compiled(&mut t), (Some(1), None, Some(2)), "compiled as typed, unsaved; and an edit on another line brings the page to it");
    t.keys("i").input("#oops(").keys("escape");
    let (_, error, _) = compiled(&mut t);
    assert!(error.is_some_and(|e| e.contains("paper.typ:3: error")), "why it doesn't compile, and where");

    t.keys("g g space j");
    assert_eq!(t.read(|e| e.tab().unwrap().view.pages.line), Some(0), "space j brings the pages to the cursor");
    assert_eq!(t.status().as_deref(), Some("no problems in this file"), "it has no server here to say");
}

#[gpui::test]
fn pages_zoom_and_scroll_across_on_their_own(cx: &mut TestAppContext) {
    use crate::actions::{ResetZoom, ZoomIn, ZoomOut};
    cx.executor().allow_parking();
    let mut t = open("x\n", &["unused"], cx);
    t.cx.update(|_, cx| {
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-=", ZoomIn, None),
            gpui::KeyBinding::new("cmd--", ZoomOut, None),
            gpui::KeyBinding::new("cmd-0", ResetZoom, None),
        ]);
        cx.on_action(|_: &ZoomIn, cx| crate::theme::zoom(1, cx));
        cx.on_action(|_: &ZoomOut, cx| crate::theme::zoom(-1, cx));
        cx.on_action(|_: &ResetZoom, cx| crate::theme::zoom(0, cx));
    });
    let pdf = t.dir.join("paper.pdf");
    std::fs::write(&pdf, two_page_pdf()).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![pdf.clone()], cx)));
    until_it_has_pages(&mut t);
    let zooms = |t: &mut Setup| {
        let pages = t.read(|e| e.tab().map(|tab| (tab.view.pages.zoom, gpui::px(tab.view.pages.left))).unwrap());
        (pages, t.cx.update(|_, cx| crate::theme::theme(cx).zoom))
    };
    let redraw = |t: &mut Setup| {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
    };

    t.keys("cmd-=");
    assert_eq!(zooms(&mut t), ((1.25, gpui::px(0.)), 1.0), "the pages zoom, the editor doesn't");
    t.keys("cmd-= cmd--");
    assert_eq!(zooms(&mut t).0.0, 1.25, "a step each way");
    t.keys("cmd-0");
    assert_eq!(zooms(&mut t).0.0, 1.0, "and back to fitting the view");

    // ⌘ and the wheel, over them.
    redraw(&mut t);
    let size = t.cx.update(|window, _| window.viewport_size());
    let middle = gpui::point(size.width / 2., size.height / 2.);
    let wheel = |t: &mut Setup, x: f32, y: f32, modifiers: gpui::Modifiers| {
        t.cx.simulate_event(gpui::ScrollWheelEvent {
            position: middle,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(x), gpui::px(y))),
            modifiers,
            touch_phase: gpui::TouchPhase::Moved,
        });
        redraw(t);
    };
    wheel(&mut t, 0., 200., gpui::Modifiers::command());
    let ((zoom, left), editor_zoom) = zooms(&mut t);
    assert!(zoom > 2., "⌘-scroll zooms in: {zoom}");
    assert!(t.cx.update(|_, cx| crate::ui::pages::tiles(cx)) > 0, "zoomed in, it's drawn in tiles");
    assert!(left > gpui::px(0.), "around the pointer, so across as well");
    assert_eq!(editor_zoom, 1.0);
    wheel(&mut t, 150., 0., gpui::Modifiers::none());
    assert_eq!(zooms(&mut t).0.1, left - gpui::px(150.), "wider than the view, they scroll across");
    wheel(&mut t, 0., -2000., gpui::Modifiers::command());
    assert_eq!(zooms(&mut t).0, (0.1, gpui::px(0.)), "out only so far, and narrower than the view there's nothing to scroll across");

    t.keys("cmd-shift-[");
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![t.dir.join("doc.txt")], cx)));
    t.keys("cmd-=");
    assert_eq!(zooms_of_editor(&mut t), 1.1, "in the code, it's the editor's zoom");
}

fn zooms_of_editor(t: &mut Setup) -> f32 {
    t.cx.update(|_, cx| crate::theme::theme(cx).zoom)
}
