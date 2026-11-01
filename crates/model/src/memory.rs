//! Not panicking the machine. On macOS a model's memory is wired: it can't be paged
//! out, and with nearly all of memory wired macOS starves until its watchdog panics
//! the machine (seen at about 15 GB of 16). So a model only loads if it fits, and if
//! memory runs out later, to whatever else is running, it's the model that goes.
//! Elsewhere it's what's in use that's counted: nothing panics, but the machine crawls.

use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::Duration;

const GB: u64 = 1 << 30;
/// What loading wires, beyond the file: the context, its KV cache and buffers.
const OVERHEAD_PERCENT: u64 = 20;
/// Memory left unwired, at the least, to load a model.
const SPARE_TO_LOAD: u64 = 2 * GB;
/// And once loaded, before the model ends itself.
const SPARE_TO_GO_ON: u64 = 5 * GB / 4;

#[cfg(target_os = "macos")]
pub fn total() -> Option<u64> {
    let output = Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

/// What can't be given to a model: what's wired.
#[cfg(target_os = "macos")]
fn wired() -> Option<u64> {
    let output = Command::new("/usr/bin/vm_stat").output().ok()?;
    parse_wired(&String::from_utf8(output.stdout).ok()?)
}

#[cfg(target_os = "linux")]
fn meminfo(key: &str) -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&text, key)
}

#[cfg(target_os = "linux")]
pub fn total() -> Option<u64> {
    meminfo("MemTotal")
}

/// What can't be given to a model: what isn't available.
#[cfg(target_os = "linux")]
fn wired() -> Option<u64> {
    Some(meminfo("MemTotal")?.saturating_sub(meminfo("MemAvailable")?))
}

#[cfg(windows)]
fn status() -> Option<windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: an all-zero MEMORYSTATUSEX is valid, and the call only fills in what it's given.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status)
}

#[cfg(windows)]
pub fn total() -> Option<u64> {
    Some(status()?.ullTotalPhys)
}

/// What can't be given to a model: what isn't available.
#[cfg(windows)]
fn wired() -> Option<u64> {
    let status = status()?;
    Some(status.ullTotalPhys.saturating_sub(status.ullAvailPhys))
}

/// `key` in the text of `/proc/meminfo`, in bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.split(':').next() == Some(key))?;
    let kb: u64 = line.split(':').nth(1)?.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(any(target_os = "macos", test))]
fn parse_wired(vm_stat: &str) -> Option<u64> {
    let page_size: u64 = vm_stat.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let line = vm_stat.lines().find(|l| l.starts_with("Pages wired down"))?;
    let pages: u64 = line.rsplit(':').next()?.trim().trim_end_matches('.').parse().ok()?;
    Some(pages * page_size)
}

/// What it says if a model of `file_size` wouldn't fit, with `wired` of `total` taken.
fn too_big(file_size: u64, wired: u64, total: u64) -> Option<String> {
    let need = file_size + file_size * OVERHEAD_PERCENT / 100;
    let room = total.saturating_sub(SPARE_TO_LOAD).saturating_sub(wired);
    let gb = |bytes: u64| bytes as f64 / GB as f64;
    (need > room).then(|| {
        format!(
            "this model needs about {:.1} GB of memory, and only {:.1} GB can be spared now: close something heavy, or set a smaller local_model",
            gb(need),
            gb(room)
        )
    })
}

#[cfg_attr(not(feature = "llama"), allow(dead_code))]
pub fn check_fits(path: &Path) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?.len();
    match (wired(), total()) {
        (Some(wired), Some(total)) => too_big(size, wired, total).map_or(Ok(()), Err),
        _ => Ok(()),
    }
}

/// Watch memory from now on, and call `stop` if it runs out.
#[cfg_attr(not(feature = "llama"), allow(dead_code))]
pub fn guard(stop: fn(&str) -> !) {
    let Some(total) = total() else { return };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if wired().is_some_and(|wired| wired + SPARE_TO_GO_ON > total) {
                stop("memory is nearly full, so the local model stopped, to keep the machine going: close something heavy, and ask again");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_meminfo() {
        let text = "MemTotal:       16000 kB\nMemFree:         100 kB\nMemAvailable:    8000 kB\n";
        assert_eq!(parse_meminfo(text, "MemTotal"), Some(16000 * 1024));
        assert_eq!(parse_meminfo(text, "MemAvailable"), Some(8000 * 1024));
        assert_eq!(parse_meminfo(text, "Nope"), None);
    }

    #[test]
    fn it_reads_vm_stat() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:      12725.\nPages wired down:      139202.\n";
        assert_eq!(parse_wired(text), Some(139202 * 16384));
    }

    #[test]
    fn a_model_loads_only_if_it_fits() {
        let total = 16 * GB;
        let fourteen_b = 8988111072;
        assert_eq!(too_big(fourteen_b, 3 * GB, total), None, "on a quiet 16 GB Mac");
        let why = too_big(fourteen_b, 8 * GB, total).expect("beside something heavy");
        assert!(why.contains("smaller local_model"), "{why}");
        assert_eq!(too_big(GB, 12 * GB, total), None, "a tiny model still fits");
        assert!(too_big(40 * GB, 0, total).is_some());
    }
}
