//! Plugins compiled to WebAssembly, and the grammars they bring: what lets a plugin
//! be one file that runs on every system, can't take the editor down, and can
//! reach only what it's handed.
//!
//! A plugin is a component of `crates/plugin-api/wit/plugin.wit`. By itself it gets
//! no files, network or environment, so all it knows is what it's asked about, and
//! all it can do is answer; what more its manifest asks for (`Access`) is mounted
//! or checked here. It's stopped if it runs too long or asks for too much memory,
//! and starts afresh the next time. Grammars are tree-sitter's own WebAssembly builds,
//! which tree-sitter runs: `ghostwrite_core` does the rest (highlights, text objects).
//! What's compiled is kept on disk between runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ghostwrite_core::syntax::Load;
use ghostwrite_page::{Bitmap, Pages, Place, Region, Renderer, Size};
use ghostwrite_plugin::manifest::Access;
use ghostwrite_plugin::{Action, Block, Code, Context, Event, Reply, Shown};
use serde_json::Value;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({ path: "../plugin-api/wit", world: "plugin" });
}

use bindings::ghostwrite::plugin::{host as wit_host, types as wit};

/// The clock's tick: how a plugin's time is measured.
const TICK: Duration = Duration::from_millis(100);
/// The most a plugin can download at a go.
const MOST_DOWNLOADED: u64 = 512 << 20;

/// Compiled code kept between runs, if there's somewhere to keep it.
fn with_cache(config: &mut Config) {
    let cache = ghostwrite_project::platform::cache_dir().and_then(|dir| {
        let mut settings = CacheConfig::new();
        settings.with_directory(dir.join("compiled-plugins"));
        Cache::new(settings).ok()
    });
    config.cache(cache);
}

/// The engine plugins run in, whose clock stops those that run too long. Grammars
/// have an engine of their own: tree-sitter doesn't expect one with such a clock.
fn plugin_engine() -> Result<&'static Engine, String> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            config.epoch_interruption(true);
            with_cache(&mut config);
            let engine = Engine::new(&config).map_err(|e| e.to_string())?;
            let ticking = engine.clone();
            std::thread::Builder::new()
                .name("plugin-clock".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(TICK);
                        ticking.increment_epoch();
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(engine)
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn grammar_engine() -> Result<&'static Engine, String> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            with_cache(&mut config);
            Engine::new(&config).map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// `path` compiled, once however many times it's started, until it changes.
fn component(path: &Path) -> Result<Component, String> {
    static COMPILED: Mutex<Option<HashMap<PathBuf, (std::time::SystemTime, Component)>>> = Mutex::new(None);
    let engine = plugin_engine()?;
    let changed = std::fs::metadata(path).and_then(|file| file.modified()).map_err(|e| e.to_string())?;
    let mut compiled = COMPILED.lock().unwrap_or_else(|e| e.into_inner());
    let compiled = compiled.get_or_insert_default();
    if let Some((when, component)) = compiled.get(path)
        && *when == changed
    {
        return Ok(component.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let component = Component::new(engine, &bytes).map_err(|e| format!("it isn't a component: {e}"))?;
    compiled.insert(path.to_path_buf(), (changed, component.clone()));
    Ok(component)
}

/// The folders fonts are in on this system.
fn font_folders() -> Vec<PathBuf> {
    let home = std::env::home_dir();
    let at_home = |rest: &str| home.as_ref().map(|home| home.join(rest));
    let mut folders: Vec<Option<PathBuf>> = if cfg!(windows) {
        vec![
            std::env::var_os("WINDIR").map(|dir| PathBuf::from(dir).join("Fonts")),
            std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Microsoft\\Windows\\Fonts")),
        ]
    } else if cfg!(target_os = "macos") {
        vec![Some("/System/Library/Fonts".into()), Some("/Library/Fonts".into()), Some("/Network/Library/Fonts".into()), at_home("Library/Fonts")]
    } else {
        vec![Some("/usr/share/fonts".into()), Some("/usr/local/share/fonts".into()), at_home(".fonts"), at_home(".local/share/fonts")]
    };
    folders.drain(..).flatten().filter(|folder| folder.is_dir()).collect()
}

/// The folder `{data}/…` or `{cache}/…` means, and where it's mounted.
fn user_folder(asked: &str) -> Option<(PathBuf, String)> {
    let parent = |dir: Option<PathBuf>| dir.and_then(|dir| dir.parent().map(Path::to_path_buf));
    if let Some(rest) = asked.strip_prefix("{data}/") {
        return Some((parent(ghostwrite_project::platform::data_dir())?.join(rest), format!("/data/{rest}")));
    }
    let rest = asked.strip_prefix("{cache}/")?;
    Some((parent(ghostwrite_project::platform::cache_dir())?.join(rest), format!("/user-cache/{rest}")))
}

/// What a running plugin has: its limit on memory, and what it's let reach.
struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    network: Vec<String>,
    /// The PATH it's been given, if it has.
    path: Option<String>,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl wit::Host for State {}

impl wit_host::Host for State {
    fn fetch(&mut self, url: String) -> Result<Vec<u8>, String> {
        let host = url.strip_prefix("https://").and_then(|rest| rest.split(['/', '?', '#']).next()).ok_or(format!("{url}: only https"))?;
        if !self.network.iter().any(|allowed| allowed == host) {
            return Err(format!("{host} isn't a host this plugin says it uses"));
        }
        // Not led anywhere else, either.
        let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).timeout_global(Some(Duration::from_secs(30))).build().into();
        let mut response = agent.get(&url).call().map_err(|e| e.to_string())?;
        response.body_mut().with_config().limit(MOST_DOWNLOADED).read_to_vec().map_err(|e| e.to_string())
    }

    fn which(&mut self, program: String) -> Option<String> {
        // A name, not a path to somewhere else.
        if program.is_empty() || program.contains(['/', '\\']) || program.starts_with('.') {
            return None;
        }
        let found = ghostwrite_project::platform::on_path(&program, std::ffi::OsStr::new(self.path.as_deref()?))?;
        Some(found.to_string_lossy().into_owned())
    }
}

/// A plugin running.
struct Live {
    store: Store<State>,
    plugin: bindings::Plugin,
    /// How long it's given for each call.
    ticks: u64,
}

impl Live {
    /// Start the plugin called `name`, with `project` mounted if it's for pages of one
    /// or its servers, and of `env`, what its access lists.
    fn start(name: &str, path: &Path, access: &Access, project: Option<&Path>, env: &[(String, String)]) -> Result<Self, String> {
        let engine = plugin_engine()?;
        let component = component(path)?;
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| e.to_string())?;
        bindings::Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).map_err(|e| e.to_string())?;
        // By default no files, sockets or environment, and its output goes nowhere.
        let mut wasi = WasiCtxBuilder::new();
        let shown: Vec<&(String, String)> = env.iter().filter(|(key, _)| access.env.contains(key)).collect();
        for (key, value) in &shown {
            wasi.env(key, value);
        }
        let mut mount = |folder: &Path, at: &str, writes: bool| {
            let perms = if writes { FsPerms::ReadWrite } else { FsPerms::ReadOnly };
            wasi.preopened_dir(folder, at, perms).map(drop).map_err(|e| format!("couldn't give it {}: {e}", folder.display()))
        };
        if let Some(project) = project.filter(|_| access.project) {
            mount(project, "/project", false)?;
        }
        if access.fonts {
            for (ix, folder) in font_folders().iter().enumerate() {
                mount(folder, &format!("/fonts/{ix}"), false)?;
            }
        }
        if access.cache {
            let own = ghostwrite_project::platform::cache_dir().ok_or("there's nowhere for its cache")?.join("plugin-data").join(name);
            std::fs::create_dir_all(&own).map_err(|e| e.to_string())?;
            mount(&own, "/cache", true)?;
        }
        for (asked, writes) in access.read.iter().map(|asked| (asked, false)).chain(access.write.iter().map(|asked| (asked, true))) {
            let (folder, at) = user_folder(asked).ok_or(format!("`{asked}` isn't anywhere here"))?;
            if writes {
                std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            }
            if folder.is_dir() {
                mount(&folder, &at, writes)?;
            }
        }
        let bytes = usize::try_from(access.memory_mb).unwrap_or(usize::MAX).saturating_mul(1 << 20);
        let state = State {
            wasi: wasi.build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(bytes).build(),
            network: access.network.clone(),
            path: shown.iter().rev().find(|(key, _)| key == "PATH").map(|(_, value)| value.clone()),
        };
        let ticks = access.seconds * 1000 / TICK.as_millis() as u64;
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(ticks);
        store.epoch_deadline_trap();
        let plugin = bindings::Plugin::instantiate(&mut store, &component, &linker).map_err(|e| format!("it doesn't fit this editor: {e}"))?;
        Ok(Self { store, plugin, ticks })
    }
}

/// Why a call into a plugin stopped.
fn why_stopped(trap: &wasmtime::Error) -> String {
    if trap.downcast_ref::<Trap>() == Some(&Trap::Interrupt) {
        "it was stopped: it took too long".to_string()
    } else {
        format!("it failed: {}", trap.root_cause())
    }
}

/// A plugin's code.
struct WasmCode {
    name: String,
    path: PathBuf,
    access: Access,
    live: Mutex<Option<Live>>,
}

impl WasmCode {
    /// Call it, starting it if it's not running. If it fails it's let go, so that the
    /// next call starts it afresh.
    fn call<T>(&self, call: impl FnOnce(&mut Live) -> wasmtime::Result<T>) -> Result<T, String> {
        let mut held = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if held.is_none() {
            *held = Some(Live::start(&self.name, &self.path, &self.access, None, &[])?);
        }
        let live = held.as_mut().expect("just started");
        live.store.set_epoch_deadline(live.ticks);
        call(live).map_err(|trap| {
            *held = None;
            why_stopped(&trap)
        })
    }

    /// Ask it what to do.
    fn ask(&self, call: impl FnOnce(&mut Live) -> wasmtime::Result<Result<Vec<wit::Action>, String>>) -> Reply {
        self.call(call)?.map(|actions| actions.into_iter().map(action).collect())
    }
}

impl Code for WasmCode {
    fn check(&self) -> Result<(), String> {
        Live::start(&self.name, &self.path, &self.access, None, &[]).map(drop)
    }

    fn run(&self, name: &str, args: &[String], context: &Context) -> Reply {
        let context = wit_context(context);
        self.ask(|live| live.plugin.call_run(&mut live.store, name, args, &context))
    }

    fn on_event(&self, event: Event, context: &Context) -> Reply {
        let event = match event {
            Event::Opened => wit::Event::Opened,
            Event::Saved => wit::Event::Saved,
            Event::Closed => wit::Event::Closed,
        };
        let context = wit_context(context);
        self.ask(|live| live.plugin.call_on_event(&mut live.store, event, &context))
    }

    fn highlight(&self, text: &str) -> Vec<(std::ops::Range<usize>, String)> {
        let Ok(spans) = self.call(|live| live.plugin.call_highlight(&mut live.store, text)) else { return vec![] };
        // It's only asked about this text: what's outside it isn't colored.
        let inside = |span: &wit::Span| span.start <= span.end && text.is_char_boundary(span.start as usize) && text.is_char_boundary(span.end as usize);
        spans.into_iter().filter(inside).map(|span| (span.start as usize..span.end as usize, span.scope)).collect()
    }

    fn renderer(&self, root: &Path, path: &Path) -> Result<Box<dyn Renderer>, String> {
        // Rooted at the project if it's in it, else at its own folder.
        let root = if path.starts_with(root) { root } else { path.parent().ok_or("no folder to make it in")? };
        let relative = path.strip_prefix(root).map_err(|_| format!("{} isn't in {}", path.display(), root.display()))?;
        Ok(Box::new(WasmRenderer {
            name: self.name.clone(),
            path: self.path.clone(),
            access: self.access.clone(),
            root: root.to_path_buf(),
            file: relative.to_string_lossy().replace('\\', "/"),
            live: Arc::new(Mutex::new(None)),
        }))
    }

    fn server_env(&self, server: &str, project: &Path, root: &Path, env: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
        let (project_said, root_said) = (project.to_string_lossy(), root.to_string_lossy());
        self.about_servers(project, env, |live| live.plugin.call_server_env(&mut live.store, server, &project_said, &root_said))
    }

    fn server_settings(&self, server: &str, project: &Path, root: &Path, env: &[(String, String)], section: &str) -> Result<Value, String> {
        let (project_said, root_said) = (project.to_string_lossy(), root.to_string_lossy());
        let said = self.about_servers(project, env, |live| live.plugin.call_server_settings(&mut live.store, server, &project_said, &root_said, section))?;
        said.map_or(Ok(Value::Null), |json| serde_json::from_str(&json).map_err(|e| format!("its settings aren't JSON: {e}")))
    }

    fn panel_open(&self, language: &str) -> Result<Shown, String> {
        self.call(|live| live.plugin.call_panel_open(&mut live.store, language)).map(shown)
    }

    fn panel_answered(&self, state: &str, answers: &[Option<Value>]) -> Result<Shown, String> {
        let answers: Vec<Option<String>> = answers.iter().map(|answer| answer.as_ref().map(Value::to_string)).collect();
        self.call(|live| live.plugin.call_panel_answered(&mut live.store, state, &answers)).map(shown)
    }

    fn panel_notification(&self, state: &str, method: &str, params: &Value) -> Result<(Shown, bool), String> {
        let params = params.to_string();
        let (panel, again) = self.call(|live| live.plugin.call_panel_notification(&mut live.store, state, method, &params))?;
        Ok((shown(panel), again))
    }
}

/// A panel as its code says it is.
fn shown(panel: wit::Panel) -> Shown {
    let block = |block: wit::Block| match block {
        wit::Block::Heading(text) => Block::Heading(text),
        wit::Block::Code(wit::CodeBlock { text, stands_out }) => Block::Code { text, stands_out },
        wit::Block::Good(text) => Block::Good(text),
        wit::Block::Muted(text) => Block::Muted(text),
    };
    Shown { state: panel.state, blocks: panel.blocks.into_iter().map(block).collect(), working_on: panel.working_on }
}

impl WasmCode {
    /// Ask it about its servers, started afresh for this project, with what of `env` it may see.
    fn about_servers<T>(&self, project: &Path, env: &[(String, String)], call: impl FnOnce(&mut Live) -> wasmtime::Result<T>) -> Result<T, String> {
        let mut live = Live::start(&self.name, &self.path, &self.access, Some(project), env)?;
        call(&mut live).map_err(|trap| why_stopped(&trap))
    }
}

/// A plugin making pages of one file: running with its project mounted.
struct WasmRenderer {
    name: String,
    path: PathBuf,
    access: Access,
    root: PathBuf,
    file: String,
    live: Arc<Mutex<Option<Live>>>,
}

impl Renderer for WasmRenderer {
    fn render(&mut self, text: &str) -> Result<Arc<dyn Pages>, String> {
        let mut held = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if held.is_none() {
            let mut started = Live::start(&self.name, &self.path, &self.access, Some(&self.root), &[])?;
            let offset = chrono::Local::now().offset().local_minus_utc() / 60;
            let Live { store, plugin, .. } = &mut started;
            plugin.call_open_pages(store, &self.file, offset).map_err(|trap| why_stopped(&trap))??;
            *held = Some(started);
        }
        let live = held.as_mut().expect("just started");
        live.store.set_epoch_deadline(live.ticks);
        let made = live.plugin.call_make_pages(&mut live.store, text);
        let made = match made {
            // It knows the project by where it's mounted; you know it by where it is.
            Ok(made) => made.map_err(|why| why.replace("/project/", &format!("{}/", self.root.display())))?,
            Err(trap) => {
                *held = None;
                return Err(why_stopped(&trap));
            }
        };
        Ok(Arc::new(MadePages { pages: made, live: self.live.clone() }))
    }
}

/// Pages a plugin made: all that's asked of them is answered here but drawing them.
struct MadePages {
    pages: Vec<wit::Page>,
    live: Arc<Mutex<Option<Live>>>,
}

impl MadePages {
    fn glyphs(&self) -> impl Iterator<Item = (usize, &wit::Glyph)> {
        self.pages.iter().enumerate().flat_map(|(ix, page)| page.glyphs.iter().map(move |glyph| (ix, glyph)))
    }

    fn place(page: usize, glyph: &wit::Glyph) -> Place {
        Place { page, x: glyph.x, y: glyph.y, width: glyph.width, height: glyph.height }
    }
}

impl Pages for MadePages {
    fn sizes(&self) -> Vec<Size> {
        self.pages.iter().map(|page| Size { width: page.width, height: page.height }).collect()
    }

    fn key(&self, page: usize) -> u128 {
        self.pages.get(page).map_or(0, |page| (u128::from(page.key_high) << 64) | u128::from(page.key_low))
    }

    fn draw(&self, page: usize, region: Region) -> Option<Bitmap> {
        let page = self.pages.get(page)?;
        if region.too_big() {
            return None;
        }
        let asked = wit::Region {
            scale: region.scale,
            x: u32::try_from(region.x).ok()?,
            y: u32::try_from(region.y).ok()?,
            width: u32::try_from(region.width).ok()?,
            height: u32::try_from(region.height).ok()?,
        };
        let mut held = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let live = held.as_mut()?;
        live.store.set_epoch_deadline(live.ticks);
        let drawn = live.plugin.call_draw(&mut live.store, page.key_high, page.key_low, asked);
        match drawn {
            // A drawing that isn't what was asked for isn't shown.
            Ok(drawn) => drawn
                .filter(|drawn| (drawn.width, drawn.height) == (asked.width, asked.height) && drawn.bgra.len() == region.width * region.height * 4)
                .map(|drawn| Bitmap { width: region.width, height: region.height, bgra: drawn.bgra }),
            Err(_) => {
                *held = None;
                None
            }
        }
    }

    fn cursor(&self, at: usize) -> Option<Place> {
        let at = at as u32;
        self.glyphs().find(|(_, glyph)| glyph.source.is_some_and(|(start, end)| start <= at && at < end)).map(|(page, glyph)| Self::place(page, glyph))
    }

    fn nearest(&self, at: usize) -> Option<Place> {
        let to = at as u32;
        self.cursor(at).or_else(|| {
            let after = self.glyphs().filter(|(_, glyph)| glyph.source.is_some_and(|(start, _)| start >= to));
            after.min_by_key(|(_, glyph)| glyph.source.map(|(start, _)| start)).map(|(page, glyph)| Self::place(page, glyph))
        })
    }

    fn covering(&self, range: std::ops::Range<usize>) -> Vec<Place> {
        let (from, to) = (range.start as u32, range.end as u32);
        self.glyphs()
            .filter(|(_, glyph)| glyph.source.is_some_and(|(start, end)| start < to && from < end))
            .map(|(page, glyph)| Self::place(page, glyph))
            .collect()
    }

    /// The glyph there, or the nearest (lines count more than columns); after it
    /// if it's on its right half.
    fn at(&self, page: usize, x: f32, y: f32) -> Option<usize> {
        let distance = |glyph: &wit::Glyph| {
            let dx = (glyph.x - x).max(x - (glyph.x + glyph.width)).max(0.);
            let dy = (glyph.y - y).max(y - (glyph.y + glyph.height)).max(0.);
            dy * 4. + dx
        };
        let glyph = self.pages.get(page)?.glyphs.iter().filter(|glyph| glyph.source.is_some()).min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        let (start, end) = glyph.source?;
        Some(if x > glyph.x + glyph.width / 2. { end } else { start } as usize)
    }
}

fn wit_context(context: &Context) -> wit::Context {
    wit::Context {
        root: context.root.clone(),
        path: context.path.clone(),
        language: context.language.clone(),
        text: context.text.clone(),
        cursor: context.cursor as u64,
        selection_start: context.selection.start as u64,
        selection_end: context.selection.end as u64,
    }
}

fn action(action: wit::Action) -> Action {
    match action {
        wit::Action::Say(text) => Action::Say(text),
        wit::Action::Edit(wit::Edit { start, end, text }) => {
            let at = |offset: u64| usize::try_from(offset).unwrap_or(usize::MAX);
            Action::Edit { range: at(start)..at(end), text }
        }
        wit::Action::Open(path) => Action::Open(path),
    }
}

/// A grammar compiled to WebAssembly: tree-sitter loads it, and its parsers run it.
struct WasmGrammar {
    name: String,
    path: PathBuf,
}

impl Load for WasmGrammar {
    /// Loaded once however many languages use it, until it changes.
    fn language(&self) -> Result<tree_sitter::Language, String> {
        static LOADED: Mutex<Option<HashMap<PathBuf, (std::time::SystemTime, tree_sitter::Language)>>> = Mutex::new(None);
        // What loads them, kept for as long as they're used.
        static LOADER: OnceLock<Result<Mutex<tree_sitter::WasmStore>, String>> = OnceLock::new();
        let changed = std::fs::metadata(&self.path).and_then(|file| file.modified()).map_err(|e| e.to_string())?;
        let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        let loaded = loaded.get_or_insert_default();
        if let Some((when, language)) = loaded.get(&self.path)
            && *when == changed
        {
            return Ok(language.clone());
        }
        let loader = LOADER
            .get_or_init(|| tree_sitter::WasmStore::new(grammar_engine()?).map(Mutex::new).map_err(|e| format!("{e:?}")))
            .as_ref()
            .map_err(Clone::clone)?;
        let bytes = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        let language = loader.lock().unwrap_or_else(|e| e.into_inner()).load_language(&self.name, &bytes).map_err(|e| format!("{e:?}"))?;
        loaded.insert(self.path.clone(), (changed, language.clone()));
        Ok(language)
    }

    fn parser(&self) -> Result<tree_sitter::Parser, String> {
        let mut parser = tree_sitter::Parser::new();
        let store = tree_sitter::WasmStore::new(grammar_engine()?).map_err(|e| format!("{e:?}"))?;
        parser.set_wasm_store(store).map_err(|e| format!("{e:?}"))?;
        Ok(parser)
    }
}

/// What runs plugins in this crate's way.
pub struct Runtime;

impl ghostwrite_plugin::Runtime for Runtime {
    fn code(&self, name: &str, path: &Path, access: &Access) -> Box<dyn Code> {
        Box::new(WasmCode { name: name.to_string(), path: path.to_path_buf(), access: access.clone(), live: Mutex::new(None) })
    }

    fn grammar(&self, name: &str, path: &Path) -> Box<dyn Load> {
        Box::new(WasmGrammar { name: name.to_string(), path: path.to_path_buf() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bindings::ghostwrite::plugin::host::Host as _;

    #[test]
    fn it_downloads_only_from_hosts_it_says_it_uses_and_only_over_https() {
        let mut state = State {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().build(),
            network: vec!["allowed.invalid".into()],
            path: None,
        };
        for (url, why) in [
            ("https://elsewhere.invalid/x", "isn't a host"),
            ("https://allowed.invalid.elsewhere.invalid/x", "isn't a host"),
            ("https://elsewhere.invalid/allowed.invalid", "isn't a host"),
            ("https://allowed.invalid@elsewhere.invalid/x", "isn't a host"),
            ("http://allowed.invalid/x", "only https"),
            ("file:///etc/hosts", "only https"),
        ] {
            let said = state.fetch(url.to_string()).unwrap_err();
            assert!(said.contains(why), "{url}: {said}");
        }
        assert!(!state.fetch("https://allowed.invalid/x".to_string()).unwrap_err().contains("isn't a host"), "that one it tries");
    }
}
