//! Whether there's a newer version: a small JSON file, `{ "version": …, "url": … }`,
//! at the address a release is built with. It only says so: what's installed
//! is up to you, and a build made without an address never goes looking.

use gpui::{AppContext as _, Context};

use super::Editor;

const FEED: Option<&str> = option_env!("GHOSTWRITE_UPDATE_FEED");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether version `a` is newer than version `b`: by their numbers, in order.
fn newer(a: &str, b: &str) -> bool {
    let numbers = |version: &str| version.trim_start_matches('v').split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    numbers(a) > numbers(b)
}

/// What the feed says, if it's newer than this: its version, and where to get it.
fn news(feed: &str) -> Result<Option<(String, String)>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).build().into();
    let body = agent.get(feed).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let latest: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let version = latest["version"].as_str().ok_or("the update feed has no version")?;
    Ok(newer(version, VERSION).then(|| (version.to_string(), latest["url"].as_str().unwrap_or_default().to_string())))
}

impl Editor {
    /// Look for a newer version. `asked`: you asked, so say what came of it either way.
    pub(super) fn check_for_update(&mut self, asked: bool, cx: &mut Context<Self>) {
        let Some(feed) = FEED.filter(|feed| !feed.is_empty()) else {
            if asked {
                self.project.say(format!("this is {VERSION}, built from source: it has no address to look for updates at"));
            }
            return;
        };
        let looking = cx.background_spawn(async move { ghostwrite_project::contained("looking for an update", || news(feed)).and_then(|news| news) });
        cx.spawn(async move |this, cx| {
            let found = looking.await;
            this.update(cx, |this, cx| {
                match found {
                    Ok(Some((version, url))) => this.project.say(format!("Ghostwrite {version} is out (this is {VERSION}): {url}")),
                    Ok(None) if asked => this.project.say(format!("{VERSION} is the latest")),
                    Err(why) if asked => this.project.say(format!("couldn't look for an update: {why}")),
                    _ => return,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn versions_go_by_their_numbers() {
        assert!(newer("0.10.0", "0.9.9"), "not by their letters");
        assert!(newer("v1.0.0", "0.2.0"));
        assert!(newer("0.2.1", "0.2"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
    }
}
