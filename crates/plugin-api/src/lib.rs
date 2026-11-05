//! What a Ghostwrite plugin written in Rust compiles against. A plugin implements
//! [`Plugin`] for a type, says so with [`register!`], and builds for WebAssembly:
//!
//! ```ignore
//! use ghostwrite_plugin_api::*;
//!
//! struct Words;
//!
//! impl Plugin for Words {
//!     fn run(_name: String, _args: Vec<String>, context: Context) -> Reply {
//!         Ok(vec![Action::Say(format!("{} words", context.text.split_whitespace().count()))])
//!     }
//! }
//!
//! register!(Words);
//! ```
//!
//! `cargo build --release --target wasm32-wasip2` makes the `.wasm`. Its
//! `plugin.json` says which commands and events it answers: see the README.
//!
//! A plugin is shown only what it's given: unless its manifest asks (`access`, which
//! is shown when it's installed) it can't reach files, the network or the
//! environment, and it's stopped if it takes too long.

#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "plugin",
        pub_export_macro: true,
        export_macro_name: "export",
        default_bindings_module: "ghostwrite_plugin_api::bindings",
    });
}

pub use bindings::ghostwrite::plugin::host::{fetch, which};
pub use bindings::ghostwrite::plugin::types::{Action, Block, CodeBlock, Context, Drawing, Edit, Event, Glyph, Page, Panel, Region, Span};

/// What a plugin answers: what to do, or what went wrong, for the status bar.
pub type Reply = Result<Vec<Action>, String>;

/// What a plugin does. Each call is a function of what it's given; what it keeps
/// from one call to the next it keeps in statics, and it's lost if the plugin
/// is stopped.
pub trait Plugin {
    /// Run a command its manifest lists.
    fn run(name: String, _args: Vec<String>, _context: Context) -> Reply {
        Err(format!("this plugin has no command {name}"))
    }

    /// An event its manifest asks for has happened.
    fn on_event(_event: Event, _context: Context) -> Reply {
        Ok(vec![])
    }

    /// What to color in `text`, in order, as its manifest's `"highlights"` says it does.
    fn highlight(_text: String) -> Vec<Span> {
        vec![]
    }

    /// Get ready to make pages of the file at `path`, as its manifest's `"pages"` says it can.
    fn open_pages(_path: String, _utc_offset_minutes: i32) -> Result<(), String> {
        Err("this plugin doesn't make pages".into())
    }

    /// Pages from the file's text as it is now.
    fn make_pages(_text: String) -> Result<Vec<Page>, String> {
        Err("this plugin doesn't make pages".into())
    }

    /// Draw `region` of the page with this key: exactly as big as it, with what's
    /// past the page's edge see-through.
    fn draw(_key_high: u64, _key_low: u64, _region: Region) -> Option<Drawing> {
        None
    }

    /// What `server` starts with, if its manifest says this plugin configures it:
    /// environment variables over those it would have. `project` is the project's
    /// folder (mounted at `/project` if its manifest asks) and `root` the folder in it
    /// the server's started in, as the editor knows them; what `access.env` lists is
    /// in `std::env`, as the server would have it.
    fn server_env(_server: String, _project: String, _root: String) -> Vec<(String, String)> {
        vec![]
    }

    /// Its answer to that server's `workspace/configuration` for `section`, as JSON.
    fn server_settings(_server: String, _project: String, _root: String, _section: String) -> Option<String> {
        None
    }

    /// The panel beside a file of `language`, as its manifest's `"panel"` says it has,
    /// before its server's been asked anything. What it keeps about the file goes in
    /// its `state`, which comes back with each call about that panel.
    fn panel_open(_language: String) -> Panel {
        Panel { state: String::new(), blocks: vec![], working_on: None }
    }

    /// The server's answers to the panel's `requests`, in that order, as JSON, each
    /// unless it failed or was empty.
    fn panel_answered(state: String, _answers: Vec<Option<String>>) -> Panel {
        Panel { state, blocks: vec![], working_on: None }
    }

    /// A notification of the server's about the file that the panel's `notifications`
    /// lists, its params as JSON; and whether to ask its requests again.
    fn panel_notification(state: String, _method: String, _params: String) -> (Panel, bool) {
        (Panel { state, blocks: vec![], working_on: None }, false)
    }
}

/// Make `$plugin`, which implements [`Plugin`], what this `.wasm` runs.
#[macro_export]
macro_rules! register {
    ($plugin:ty) => {
        struct __Registered;

        impl $crate::bindings::Guest for __Registered {
            fn run(name: String, args: Vec<String>, context: $crate::Context) -> $crate::Reply {
                <$plugin as $crate::Plugin>::run(name, args, context)
            }

            fn on_event(event: $crate::Event, context: $crate::Context) -> $crate::Reply {
                <$plugin as $crate::Plugin>::on_event(event, context)
            }

            fn highlight(text: String) -> Vec<$crate::Span> {
                <$plugin as $crate::Plugin>::highlight(text)
            }

            fn open_pages(path: String, utc_offset_minutes: i32) -> Result<(), String> {
                <$plugin as $crate::Plugin>::open_pages(path, utc_offset_minutes)
            }

            fn make_pages(text: String) -> Result<Vec<$crate::Page>, String> {
                <$plugin as $crate::Plugin>::make_pages(text)
            }

            fn draw(key_high: u64, key_low: u64, region: $crate::Region) -> Option<$crate::Drawing> {
                <$plugin as $crate::Plugin>::draw(key_high, key_low, region)
            }

            fn server_env(server: String, project: String, root: String) -> Vec<(String, String)> {
                <$plugin as $crate::Plugin>::server_env(server, project, root)
            }

            fn server_settings(server: String, project: String, root: String, section: String) -> Option<String> {
                <$plugin as $crate::Plugin>::server_settings(server, project, root, section)
            }

            fn panel_open(language: String) -> $crate::Panel {
                <$plugin as $crate::Plugin>::panel_open(language)
            }

            fn panel_answered(state: String, answers: Vec<Option<String>>) -> $crate::Panel {
                <$plugin as $crate::Plugin>::panel_answered(state, answers)
            }

            fn panel_notification(state: String, method: String, params: String) -> ($crate::Panel, bool) {
                <$plugin as $crate::Plugin>::panel_notification(state, method, params)
            }
        }

        $crate::bindings::export!(__Registered);
    };
}
