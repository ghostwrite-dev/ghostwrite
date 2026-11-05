# Install

## macOS

Download the DMG from [the latest release](https://github.com/ghostwrite-dev/ghostwrite/releases/latest), open it, and drag Ghostwrite to Applications.

The app isn't notarized yet. The first time you open it, right-click it in Applications and choose **Open**, then confirm. After that it opens normally.

## From source

Building needs [Rust](https://rustup.rs) 1.97 (what CI builds with). Clone the repository, then run the script for your system from its root:

```sh
git clone https://github.com/ghostwrite-dev/ghostwrite
cd ghostwrite

./install.sh                    # macOS: installs /Applications/Ghostwrite.app
scripts/install-linux.sh        # Linux: installs ~/.local/bin/ghostwrite and a desktop entry
scripts/install-windows.ps1     # Windows: installs for this user, with a Start menu entry
```

Each script builds the app in release mode, downloads the plugins that come with it (as `plugins.lock` pins them, each checked against its sha256), and installs both. Typst and Lean aren't among them: install them from the editor with `:plugin install ghostwrite-dev/typst` and `:plugin install ghostwrite-dev/lean`.

To install the app with none of the plugins, add `--no-plugins` (`-NoPlugins` on Windows). Only plain text is built in, so then pick the languages you want from the editor, one at a time from the default plugins' release:

```text
:plugin install ghostwrite-dev/default-plugins/rust
```

What else each system needs:

| System | Needs |
| --- | --- |
| macOS | Xcode's command line tools (with the Metal toolchain) and `cmake` |
| Linux | `cmake`, a C compiler, and the development files for Vulkan, X11, Wayland and fontconfig. On Debian and Ubuntu: `apt install cmake clang libvulkan-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libssl-dev` |
| Windows | Rust with the MSVC toolchain, `cmake`, and LLVM (for llama.cpp's bindings) |

The plugins come already built for WebAssembly, so building the app doesn't build them. To build them from a checkout of [default-plugins](https://github.com/ghostwrite-dev/default-plugins) instead, see [Contributing](contributing.md#plugins-in-the-build).

### Without local models

`cmake` and a C++ compiler are only needed for llama.cpp, which runs [local models](models/local.md). To build without it:

```sh
cargo build --release --no-default-features --features wasm-plugins
```

## How far each system has come

macOS is what Ghostwrite is used on every day. Windows and Linux are type-checked and tested in CI, so expect rough edges. They differ in a few ways:

- The app's keys use Ctrl+Shift where a Mac has ⌘ (Ctrl+Shift+T for ⌘T), Ctrl+Alt+Shift where it has ⌘⇧, and Ctrl+Alt where it has ⌘⌥. Plain Ctrl belongs to the editor and the shell. `space ?` shows the keys as they are on your system.
- A local model runs on the CPU, so it's slower. Metal is the only GPU it uses so far.
- The terminal is PowerShell on Windows (`pwsh` if it's installed).
- Settings live in `%APPDATA%\Ghostwrite` on Windows and `~/.config/ghostwrite` elsewhere. On Linux, `$XDG_CONFIG_HOME/ghostwrite` if that's set. These docs write `~/.config/ghostwrite` for all of them.

## First launch

Ghostwrite opens on the projects page. With no recent projects it suggests where to start:

```text
:tutor        learn the keys, in a few minutes
:api-key      give it a Claude API key, or use Claude Code if it's installed
:model        see which models can be asked, and choose one
space ?       see every key, at any time
```

Press `o` (or ⌘⇧O) to open a folder. You can also start it from a shell with a folder or files to open:

```sh
ghostwrite ~/code/my-project
```

To ask the model anything you need one backend: an [API key](models/api.md), [Claude Code](models/claude-code.md) installed, or a [local model](models/local.md). The editor works without any of them.

## Updating

`:update` checks whether there's a newer version and tells you. It never installs anything by itself.

- **DMG:** download the new one and replace the app in Applications.
- **From source:** pull, and run the install script again. It replaces the installed app and its bundled plugins.

```sh
git pull
./install.sh
```

Your settings, plugins you installed yourself (Typst and Lean among them), and what Ghostwrite remembers about your projects are kept elsewhere, so updating doesn't touch them.
