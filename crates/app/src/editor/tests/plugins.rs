//! Plugins as the editor sees them: commands, events, and what comes of their answers.

use std::sync::Mutex;

use ghostwrite_plugin::{Action, Context as Shown, Event, Plugin, PluginCommand, Reply};
use gpui::TestAppContext;

use super::*;
use crate::editor::commands::{Command, complete, parse};
use crate::editor::plugin_host::apply_edits;

/// What the test plugin was told, in order.
static TOLD: Mutex<Vec<(Event, Option<String>, String)>> = Mutex::new(vec![]);

struct Shouting;

impl Plugin for Shouting {
    fn name(&self) -> &str {
        "shouting"
    }

    fn commands(&self) -> &'static [PluginCommand] {
        &[
            PluginCommand { name: "shout-test", what: "" },
            PluginCommand { name: "fail-test", what: "" },
            PluginCommand { name: "bad-edit-test", what: "" },
            PluginCommand { name: "leave-test", what: "" },
            PluginCommand { name: "panic-test", what: "" },
            // The editor's: it stays the editor's.
            PluginCommand { name: "write", what: "" },
        ]
    }

    fn run(&self, name: &str, args: &[String], context: &Shown) -> Reply {
        match name {
            "shout-test" => Ok(vec![
                Action::Edit { range: context.selection.clone(), text: context.text[context.selection.clone()].to_uppercase() },
                Action::Say(format!("shouted {}", args.join("+"))),
            ]),
            "fail-test" => Err("it went wrong".into()),
            "bad-edit-test" => Ok(vec![Action::Edit { range: 0..context.text.len() + 1, text: "gone".into() }]),
            "leave-test" => Ok(vec![Action::Open("../outside.txt".into())]),
            "panic-test" => panic!("on purpose"),
            _ => Ok(vec![]),
        }
    }

    fn events(&self) -> &'static [Event] {
        &[Event::Opened, Event::Saved, Event::Closed]
    }

    fn on_event(&self, event: Event, context: &Shown) -> Reply {
        TOLD.lock().unwrap().push((event, context.path.clone(), context.text.clone()));
        Ok(vec![])
    }
}

/// An editor, with the test plugin there.
fn with_plugin<'a>(text: &str, cx: &'a mut TestAppContext) -> Setup<'a> {
    static ADDED: std::sync::Once = std::sync::Once::new();
    ADDED.call_once(|| ghostwrite_plugin::add(Box::new(Shouting)));
    open(text, &[], cx)
}

fn settle(t: &mut Setup) {
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
}

#[gpui::test]
fn a_plugins_command_edits_as_one_undo_step_and_says_so(cx: &mut TestAppContext) {
    let mut t = with_plugin("say hello now\n", cx);
    t.keys("w w");
    t.command("shout-test a b");
    settle(&mut t);
    assert_eq!(t.text(), "say HELLO now\n");
    assert_eq!(t.status().as_deref(), Some("shouting: shouted a+b"), "what it says is marked as its own");
    t.keys("escape u");
    assert_eq!(t.text(), "say hello now\n", "one undo takes it back");
}

#[gpui::test]
fn what_a_plugin_gets_wrong_is_said_and_changes_nothing(cx: &mut TestAppContext) {
    let mut t = with_plugin("text\n", cx);
    t.command("fail-test");
    settle(&mut t);
    assert_eq!(t.status().as_deref(), Some("shouting: it went wrong"));
    t.command("bad-edit-test");
    settle(&mut t);
    assert!(t.status().is_some_and(|said| said.contains("isn't in the text")), "{:?}", t.status());
    t.command("leave-test");
    settle(&mut t);
    assert!(t.status().is_some_and(|said| said.contains("isn't in this project")), "{:?}", t.status());
    t.command("panic-test");
    settle(&mut t);
    assert!(t.status().is_some_and(|said| said.contains("on purpose")), "a panic is contained: {:?}", t.status());
    assert_eq!(t.text(), "text\n");
}

#[gpui::test]
fn an_edit_of_text_thats_moved_on_isnt_made(cx: &mut TestAppContext) {
    let mut t = with_plugin("text\n", cx);
    let shown = t.read(|e| {
        let tab = e.tab().unwrap();
        (tab.doc.id, tab.doc.buffer.version())
    });
    t.keys("i x escape");
    let editor = t.editor.clone();
    t.cx.update(|_, cx| {
        editor.update(cx, |e, cx| e.plugin_answered("late", Some(shown), Ok(vec![Action::Edit { range: 0..4, text: "gone".into() }]), cx));
    });
    assert_eq!(t.text(), "xtext\n");
    assert!(t.status().is_some_and(|said| said.starts_with("late: the file changed")), "{:?}", t.status());
}

#[gpui::test]
fn edits_are_held_to_the_text_and_the_selection_moves_with_them(cx: &mut TestAppContext) {
    let mut t = with_plugin("ab é cd\n", cx);
    let editor = t.editor.clone();
    let edit = |t: &mut Setup, edits: Vec<(std::ops::Range<usize>, String)>| {
        let editor = editor.clone();
        t.cx.update(|_, cx| editor.update(cx, |e, _| apply_edits(e.project.tab_mut().unwrap(), edits)))
    };
    assert!(edit(&mut t, vec![(3..4, "x".into())]).is_err(), "in the middle of a character");
    assert!(edit(&mut t, vec![(0..2, "x".into()), (1..3, "y".into())]).unwrap_err().contains("overlap"));
    assert_eq!(t.text(), "ab é cd\n");
    // In any order, each about the text as it was.
    edit(&mut t, vec![(6..8, "CD".into()), (0..2, "wxyz".into())]).unwrap();
    assert_eq!(t.text(), "wxyz é CD\n");
    t.keys("u");
    assert_eq!(t.text(), "ab é cd\n", "one step");
    let version = t.read(|e| e.tab().unwrap().doc.buffer.version());
    edit(&mut t, vec![(0..2, "ab".into())]).unwrap();
    assert_eq!(t.read(|e| e.tab().unwrap().doc.buffer.version()), version, "changing nothing changes nothing");
}

#[gpui::test]
fn plugins_hear_of_files_opening_saving_and_closing(cx: &mut TestAppContext) {
    let mut t = with_plugin("x\n", cx);
    let file = t.dir.join("heard.txt");
    std::fs::write(&file, "from disk\n").unwrap();
    let path = file.display().to_string();
    let heard = |event: Event| {
        TOLD.lock().unwrap().iter().filter(|(e, p, _)| *e == event && p.as_deref() == Some(path.as_str())).map(|(_, _, text)| text.clone()).collect::<Vec<_>>()
    };

    t.command(&format!("open {path}"));
    settle(&mut t);
    assert_eq!(heard(Event::Opened), ["from disk\n"]);
    t.keys("i n e w escape");
    t.command("write");
    settle(&mut t);
    assert_eq!(heard(Event::Saved), ["newfrom disk\n"], "with the text as saved");
    t.command("buffer-close");
    settle(&mut t);
    assert_eq!(heard(Event::Closed), ["newfrom disk\n"]);
}

#[gpui::test]
fn the_editors_commands_stay_its_own_and_a_plugins_complete(cx: &mut TestAppContext) {
    let t = with_plugin("x\n", cx);
    assert!(matches!(parse("write"), Ok(Command::Write { .. })), "a plugin can't take over :write");
    assert_eq!(parse("shout-test a"), Ok(Command::FromPlugin { name: "shout-test".into(), args: vec!["a".into()] }));
    assert_eq!(complete("shout-t", &t.dir).0, "shout-test ");
    assert_eq!(complete("plugin ins", &t.dir).0, "plugin install ");
    assert_eq!(complete("plugin remove shou", &t.dir).0, "plugin remove shouting");
    assert!(parse("plugin frob").is_err());
}

#[gpui::test]
fn a_plugin_is_installed_and_removed_from_inside_the_editor(cx: &mut TestAppContext) {
    let mut t = with_plugin("x\n", cx);
    let plugins = ghostwrite_project::scratch_dir("editor-plugins");
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, _| e.plugins_at = Some(plugins.clone())));
    let source = t.dir.join("mine");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("plugin.json"), r#"{ "name": "mine-test", "version": "7", "extensions": ["minetest"], "server": ["mine-server"] }"#).unwrap();

    t.command("plugin install mine");
    settle(&mut t);
    let said = t.status().unwrap_or_default();
    assert!(said.starts_with("installed mine-test") && said.contains("version 7") && said.contains("starts `mine-server` with your permissions"), "{said}");
    assert!(plugins.join("mine-test/plugin.json").is_file());
    assert_eq!(ghostwrite_core::Language::for_path(std::path::Path::new("a.minetest")).map(|l| l.name), Some("mine-test"), "it's there at once");
    t.command("plugins");
    assert!(t.status().is_some_and(|said| said.contains("mine-test (version 7")), "{:?}", t.status());

    std::fs::write(source.join("plugin.json"), r#"{ "name": "mine-test", "version": "8", "extensions": ["minetest"] }"#).unwrap();
    t.command("plugin update mine-test");
    settle(&mut t);
    assert!(t.status().is_some_and(|said| said.starts_with("updated mine-test (version 8")), "{:?}", t.status());

    t.command("plugin remove mine-test");
    settle(&mut t);
    assert_eq!(t.status().as_deref(), Some("removed mine-test"));
    assert!(ghostwrite_core::Language::for_path(std::path::Path::new("a.minetest")).is_none(), "and gone at once");
    assert!(!plugins.join("mine-test").exists());
}
