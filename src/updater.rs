//! Self-update from GitHub Releases: download Framegrab.exe + .sha256, verify, swap in place
//! while idle, restart in the background. A missing or mismatched checksum never installs.

use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::time::Duration;
use windows::Win32::Foundation::HWND;

pub const REPO: &str = "JustSad42/framegrab";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TIMER_ID: usize = 40;

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Ready { version: String, path: std::path::PathBuf },
    Error(String),
}

pub static STATE: Mutex<State> = Mutex::new(State::Idle);

fn parse(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim_start_matches('v').split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn newer(remote: &str, local: &str) -> bool {
    parse(remote) > parse(local)
}

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}
#[derive(serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into()
}

fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut r = agent().get(url).header("User-Agent", "Framegrab-updater").call().map_err(|e| e.to_string())?;
    r.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())
}

pub fn check() -> State {
    *STATE.lock().unwrap() = State::Checking;
    let res = (|| -> Result<State, String> {
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let mut resp = agent().get(&url).header("User-Agent", "Framegrab-updater").header("Accept", "application/vnd.github+json").call().map_err(|e| {
            if e.to_string().contains("404") { "No releases published yet".to_string() } else { e.to_string() }
        })?;
        let rel: Release = resp.body_mut().read_json().map_err(|e| e.to_string())?;
        if !newer(&rel.tag_name, VERSION) {
            return Ok(State::UpToDate);
        }
        let version = rel.tag_name.trim_start_matches('v').to_string();
        let exe = rel.assets.iter().find(|a| a.name.eq_ignore_ascii_case("Framegrab.exe")).ok_or("Release has no Framegrab.exe")?;
        let sum = rel.assets.iter().find(|a| a.name.eq_ignore_ascii_case("Framegrab.exe.sha256")).ok_or("Release has no checksum")?;
        let expected = String::from_utf8_lossy(&get_bytes(&sum.browser_download_url, 4096)?).split_whitespace().next().unwrap_or("").to_lowercase();
        if expected.len() != 64 {
            return Err("Release checksum is malformed".into());
        }
        let bytes = get_bytes(&exe.browser_download_url, 200 << 20)?;
        let actual = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        if actual != expected {
            return Err("Downloaded update failed the checksum, discarded".into());
        }
        let dir = crate::config::data_dir().join("update");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("Framegrab-{version}.exe"));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        Ok(State::Ready { version, path })
    })();
    let st = res.unwrap_or_else(State::Error);
    *STATE.lock().unwrap() = st.clone();
    st
}

/// Replaces the running exe with the downloaded one and starts it. The caller then exits.
pub fn apply(path: &std::path::Path) -> Result<(), String> {
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = cur.with_extension("old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&cur, &old).map_err(|e| format!("Could not replace the app: {e}"))?;
    if let Err(e) = std::fs::copy(path, &cur) {
        let _ = std::fs::rename(&old, &cur);
        return Err(format!("Could not install the update: {e}"));
    }
    let _ = std::fs::remove_file(path);
    std::process::Command::new(&cur).arg("--background").arg("--updated").spawn().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn cleanup() {
    if let Ok(cur) = std::env::current_exe() {
        let _ = std::fs::remove_file(cur.with_extension("old.exe"));
    }
}

/// Checks 60 s after start and then every 6 h; installs on the host timer while idle.
pub fn spawn_auto(host: HWND, enabled: bool) {
    if !enabled || crate::autostart::is_dev_build() || crate::config::test_mode() {
        return;
    }
    std::thread::Builder::new()
        .name("updater".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(60));
            loop {
                if !matches!(*STATE.lock().unwrap(), State::Ready { .. }) {
                    check();
                }
                std::thread::sleep(Duration::from_secs(6 * 3600));
            }
        })
        .ok();
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(host), TIMER_ID, 30_000, None);
    }
}

pub fn on_timer(_host: HWND) {
    let st = STATE.lock().unwrap().clone();
    if let State::Ready { path, .. } = st {
        if crate::app::is_idle() && apply(&path).is_ok() {
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert!(super::newer("v1.2.0", "1.1.9"));
        assert!(super::newer("1.10.0", "1.9.0"));
        assert!(!super::newer("v1.0.0", "1.0.0"));
    }
}
