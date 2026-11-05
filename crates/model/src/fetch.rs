//! Where a model is: a `.gguf` file, or Hugging Face's `user/repo:QUANTIZATION`,
//! already downloaded by llama.cpp or by us, or downloaded now (a cut off
//! download goes on where it stopped).

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const SITE: &str = "https://huggingface.co";
/// Disk left free after a download.
#[cfg(not(test))]
const SPARE_DISK: u64 = 2 << 30;
/// In tests, only what's downloaded: they shouldn't hang on how full the disk is.
#[cfg(test)]
const SPARE_DISK: u64 = 0;
const BUFFER: usize = 1 << 20;

pub fn path_of(model: &str, status: &mut dyn FnMut(String)) -> Result<PathBuf, String> {
    // A file: not always named so (Ollama's aren't).
    if model.ends_with(".gguf") || model.starts_with('/') {
        let path = PathBuf::from(model);
        return if path.is_file() { Ok(path) } else { Err(format!("there's no model at {model}")) };
    }
    let (repo, quantization) = model.split_once(':').unwrap_or((model, "Q4_K_M"));
    if !repo.contains('/') {
        return Err(format!("local_model {model} isn't a .gguf file or user/repo:QUANTIZATION on Hugging Face"));
    }
    let home = std::env::home_dir().ok_or("no home folder")?;
    let folder = repo.replace('/', "--");
    let ours = ghostwrite_project::platform::cache_dir().ok_or("no cache folder")?.join("models").join(&folder);
    let snapshots = home.join(".cache/huggingface/hub").join(format!("models--{folder}/snapshots"));
    let cached = std::fs::read_dir(&snapshots).into_iter().flatten().flatten().map(|snapshot| snapshot.path()).chain([ours.clone()]);
    if let Some(found) = cached.filter_map(|dir: PathBuf| find_in(&dir, quantization)).next() {
        return Ok(found);
    }
    status(format!("finding {model}"));
    let file = file_in_repo(SITE, repo, quantization)?;
    std::fs::create_dir_all(&ours).map_err(|e| e.to_string())?;
    let dest = ours.join(file.rsplit('/').next().unwrap_or(&file));
    download(&format!("{SITE}/{repo}/resolve/main/{file}"), &dest, status)?;
    Ok(dest)
}

fn matches(file: &str, quantization: &str) -> bool {
    file.ends_with(".gguf") && file.to_lowercase().contains(&quantization.to_lowercase())
}

fn find_in(dir: &Path, quantization: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| matches(n, quantization)))
}

fn file_in_repo(site: &str, repo: &str, quantization: &str) -> Result<String, String> {
    let url = format!("{site}/api/models/{repo}");
    let mut response = ureq::get(&url).call().map_err(|e| format!("couldn't find {repo} on Hugging Face: {e}"))?;
    let info: serde_json::Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    let files = info["siblings"].as_array().into_iter().flatten().filter_map(|s| s["rfilename"].as_str());
    files.into_iter().find(|f| matches(f, quantization)).map(str::to_string).ok_or_else(|| format!("{repo} has no {quantization} model"))
}

#[cfg(unix)]
fn free_disk(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a C string, and statvfs(3) fills in `stats`, read only if it succeeds.
    (unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } == 0).then(|| {
        // SAFETY: it succeeded.
        let stats = unsafe { stats.assume_init() };
        // Their types differ from one Unix to another.
        #[allow(clippy::unnecessary_cast)]
        let free = stats.f_bavail as u64 * stats.f_frsize as u64;
        free
    })
}

#[cfg(windows)]
fn free_disk(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: `wide` ends in a NUL, and the call fills in `free` for as long as it runs.
    let done = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (done != 0).then_some(free)
}

fn download(url: &str, dest: &Path, status: &mut dyn FnMut(String)) -> Result<(), String> {
    let part = dest.with_extension("gguf.part");
    let name = dest.file_stem().unwrap_or_default().display().to_string();
    let have = std::fs::metadata(&part).map_or(0, |m| m.len());
    let mut response = ureq::get(url).header("Range", &format!("bytes={have}-")).call().map_err(|e| format!("couldn't download {name}: {e}"))?;
    // The server may not go on from the middle: then it's from the start.
    let resuming = response.status() == 206;
    let have = if resuming { have } else { 0 };
    let remaining = response.body().content_length().unwrap_or(0);
    let free = dest.parent().and_then(free_disk).unwrap_or(u64::MAX);
    if remaining + SPARE_DISK > free {
        return Err(format!("{name} needs {} GB of disk, and {} GB is free", (remaining + SPARE_DISK) >> 30, free >> 30));
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(resuming).write(true).truncate(!resuming).open(&part).map_err(|e| e.to_string())?;
    let total = have + remaining;
    let mut body = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buffer = vec![0; BUFFER];
    let mut got = have;
    let mut said = u64::MAX;
    loop {
        let n = body.read(&mut buffer).map_err(|e| format!("downloading {name} stopped: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        let percent = (got * 100).checked_div(total).unwrap_or(0);
        if percent != said {
            status(format!("downloading {name}: {percent}%"));
            said = percent;
        }
    }
    if total != 0 && got != total {
        return Err(format!("downloading {name} stopped at {} of {} MB: ask again to go on", got >> 20, total >> 20));
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader};
    use std::net::TcpListener;

    use super::*;

    /// A server of `body`, honouring a Range from where it asks, once per entry
    /// of `cut`: that request's connection closes after so many bytes.
    fn serve(body: &'static [u8], cut: Vec<Option<usize>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for cut in cut {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut from = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(range) = line.to_lowercase().strip_prefix("range: bytes=") {
                        from = range.trim().trim_end_matches('-').parse().unwrap();
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let rest = &body[from..];
                let mut stream = stream;
                write!(stream, "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", rest.len()).unwrap();
                stream.write_all(&rest[..cut.unwrap_or(rest.len())]).unwrap();
            }
        });
        url
    }

    #[test]
    fn a_cut_off_download_goes_on_where_it_stopped() {
        let dir = ghostwrite_project::scratch_dir("fetch");
        let dest = dir.join("tiny.gguf");
        let url = serve(b"0123456789", vec![Some(4), None]);
        let error = download(&url, &dest, &mut |_| {}).unwrap_err();
        assert!(error.contains("stop"), "{error}");
        assert!(!dest.exists(), "only a whole model has its name");
        let mut said = vec![];
        download(&url, &dest, &mut |s| said.push(s)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789");
        assert_eq!(said.last().unwrap(), "downloading tiny: 100%");
        assert!(!dir.join("tiny.gguf.part").exists());
    }

    #[test]
    fn it_finds_the_quantization_asked_for() {
        assert!(matches("Model-Q4_K_M.gguf", "q4_k_m"));
        assert!(!matches("Model-Q8_0.gguf", "Q4_K_M"));
        assert!(!matches("Q4_K_M.txt", "Q4_K_M"));
        let dir = ghostwrite_project::scratch_dir("fetch-find");
        std::fs::write(dir.join("Model-Q8_0.gguf"), "").unwrap();
        std::fs::write(dir.join("Model-Q4_K_M.gguf"), "").unwrap();
        assert_eq!(find_in(&dir, "Q4_K_M"), Some(dir.join("Model-Q4_K_M.gguf")));
    }

    #[test]
    fn a_file_that_isnt_there_says_so() {
        assert!(path_of("/nowhere/tiny.gguf", &mut |_| {}).unwrap_err().contains("no model"));
        assert!(path_of("not-a-repo", &mut |_| {}).unwrap_err().contains("isn't a .gguf"));
    }
}
