//! The Claude API key, kept where the system keeps secrets (`:api-key`), so that the app
//! needs no environment variable, which one opened from a desktop wouldn't see: the macOS
//! Keychain, the Windows Credential Manager, or on Linux the Secret Service, by way of
//! `secret-tool`. Where a program is told it, that's on its standard input, not its
//! command line, where any process could read it.

use std::sync::Mutex;

const SERVICE: &str = "app.ghostwrite.Ghostwrite.anthropic-api-key";
const ACCOUNT: &str = "default";

/// What the system has, once it's been asked: it can take a process each time.
static KEY: Mutex<Option<Option<String>>> = Mutex::new(None);

/// Whether `key` could be a key: one word of plain characters, nothing a shell-like line would split.
pub(crate) fn plausible(key: &str) -> bool {
    key.len() >= 20 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The key saved earlier, if any.
pub fn saved() -> Option<String> {
    let mut cached = KEY.lock().unwrap_or_else(|e| e.into_inner());
    cached.get_or_insert_with(|| system::read().filter(|key| !key.is_empty())).clone()
}

/// Keep `key` where the system keeps secrets.
pub fn save(key: &str) -> Result<(), String> {
    let key = key.trim();
    if !plausible(key) {
        return Err("that doesn't look like an API key".into());
    }
    system::write(key)?;
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Some(key.to_string()));
    Ok(())
}

/// Take the key out of where it's kept.
pub fn forget() {
    system::remove();
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
}

#[cfg(target_os = "macos")]
mod system {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    use super::{ACCOUNT, SERVICE};

    const SECURITY: &str = "/usr/bin/security";

    pub fn read() -> Option<String> {
        let out = Command::new(SECURITY).args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"]).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn write(key: &str) -> Result<(), String> {
        let mut security = Command::new(SECURITY)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't reach the Keychain: {e}"))?;
        let line = format!("add-generic-password -U -s {SERVICE} -a {ACCOUNT} -l \"Ghostwrite: Anthropic API key\" -w {key}\n");
        security.stdin.take().ok_or("couldn't reach the Keychain")?.write_all(line.as_bytes()).map_err(|e| format!("couldn't reach the Keychain: {e}"))?;
        let done = security.wait().map_err(|e| format!("couldn't reach the Keychain: {e}"))?;
        done.success().then_some(()).ok_or_else(|| "the Keychain didn't take it".to_string())
    }

    pub fn remove() {
        Command::new(SECURITY).args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT]).stdout(Stdio::null()).stderr(Stdio::null()).status().ok();
    }
}

#[cfg(windows)]
mod system {
    use super::{ACCOUNT, SERVICE};

    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("couldn't reach the Credential Manager: {e}"))
    }

    pub fn read() -> Option<String> {
        entry().ok()?.get_password().ok()
    }

    pub fn write(key: &str) -> Result<(), String> {
        entry()?.set_password(key).map_err(|e| format!("the Credential Manager didn't take it: {e}"))
    }

    pub fn remove() {
        if let Ok(entry) = entry() {
            entry.delete_credential().ok();
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod system {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    use super::{ACCOUNT, SERVICE};

    const TOOL: &str = "secret-tool";

    pub fn read() -> Option<String> {
        let out = Command::new(TOOL).args(["lookup", "service", SERVICE, "account", ACCOUNT]).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn write(key: &str) -> Result<(), String> {
        let needs = |e: std::io::Error| format!("keeping a key takes `secret-tool` (libsecret-tools), or set ANTHROPIC_API_KEY: {e}");
        let mut tool = Command::new(TOOL)
            .args(["store", "--label=Ghostwrite: Anthropic API key", "service", SERVICE, "account", ACCOUNT])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(needs)?;
        tool.stdin.take().ok_or("couldn't reach secret-tool")?.write_all(key.as_bytes()).map_err(needs)?;
        let done = tool.wait().map_err(needs)?;
        done.success().then_some(()).ok_or_else(|| "secret-tool didn't take it: is a keyring running?".to_string())
    }

    pub fn remove() {
        Command::new(TOOL).args(["clear", "service", SERVICE, "account", ACCOUNT]).stdout(Stdio::null()).stderr(Stdio::null()).status().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_could_be_a_key_gets_as_far_as_the_system() {
        assert!(plausible("sk-ant-api03-abcdefghijklmnop_QRST"));
        for not in ["", "short", "two words that are long enough", "key\"; delete-keychain login", "line\nbreak-long-enough-to-pass"] {
            assert!(!plausible(not), "{not}");
            assert!(save(not).is_err());
        }
    }
}
