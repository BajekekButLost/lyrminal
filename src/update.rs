use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

pub const REPO: &str = "BajekekButLost/lyrminal";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_BINARY: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub url: String,
    pub summary: String,
    pub asset: Option<Asset>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: u64, total: u64 },
    Installed { release: Release, path: PathBuf },
    Failed(String),
}

impl Status {
    pub fn busy(&self) -> bool {
        matches!(self, Status::Checking | Status::Downloading { .. })
    }

    pub fn value(&self) -> String {
        match self {
            Status::Idle => "check ↵".into(),
            Status::Checking => "checking…".into(),
            Status::UpToDate => "up to date · check ↵".into(),
            Status::Available(r) if r.asset.is_some() => format!("v{} available · install ↵", r.version),
            Status::Available(r) => format!("v{} available", r.version),
            Status::Downloading { done, total, .. } if *total > 0 => {
                format!("downloading {}%", (done * 100 / total).min(100))
            }
            Status::Downloading { .. } => "downloading…".into(),
            Status::Installed { release, .. } => format!("v{} installed · restart ↵", release.version),
            Status::Failed(_) => "failed · retry ↵".into(),
        }
    }

    pub fn desc(&self) -> String {
        let notes = |r: &Release| {
            if r.summary.is_empty() {
                format!("Release notes: {}", r.url)
            } else {
                format!("{} Release notes: {}", r.summary, r.url)
            }
        };
        match self {
            Status::Idle => format!("You have Lyrminal v{CURRENT}. Enter looks for a newer release on GitHub."),
            Status::Checking => "Looking for a newer release on GitHub…".into(),
            Status::UpToDate => format!("Lyrminal v{CURRENT} is the latest release. Enter checks again."),
            Status::Available(r) if r.asset.is_some() => format!(
                "Lyrminal v{} is out (you have v{CURRENT}). Enter downloads it and replaces this copy. {}",
                r.version,
                notes(r)
            ),
            Status::Available(r) => format!(
                "Lyrminal v{} is out, but there is no prebuilt binary for this system. Rerun install.sh with --from-source to build it. {}",
                r.version,
                notes(r)
            ),
            Status::Downloading { release, .. } => format!("Downloading Lyrminal v{}…", release.version),
            Status::Installed { release, path } => format!(
                "Lyrminal v{} is installed at {}. Enter restarts into it now, or it starts the next time you open Lyrminal.",
                release.version,
                path.display()
            ),
            Status::Failed(e) => format!("The update failed: {e}. Enter tries again."),
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Status::Available(r) => Some(format!("update v{}", r.version)),
            Status::Downloading { .. } => Some("updating…".into()),
            Status::Installed { .. } => Some("restart to update".into()),
            _ => None,
        }
    }
}

pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

pub fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-linux"),
        _ => None,
    }
}

pub fn asset_name(tag: &str, target: &str) -> String {
    format!("lyrminal-{tag}-{target}")
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

fn summary(body: &str) -> String {
    let first = body
        .split("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty() && !p.starts_with('#'))
        .unwrap_or("");
    let line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 160 {
        let cut: String = line.chars().take(159).collect();
        format!("{}…", cut.trim_end())
    } else {
        line
    }
}

fn release_from(gh: GhRelease, target: Option<&str>) -> Option<Release> {
    if gh.draft || gh.prerelease {
        return None;
    }
    let version = gh.tag_name.trim_start_matches(['v', 'V']).to_string();
    parse_version(&version)?;
    let asset = target.and_then(|t| {
        let want = asset_name(&gh.tag_name, t);
        gh.assets.into_iter().find(|a| a.name == want).map(|a| Asset {
            name: a.name,
            url: a.browser_download_url,
            size: a.size,
        })
    });
    Some(Release {
        version,
        summary: summary(gh.body.as_deref().unwrap_or("")),
        tag: gh.tag_name,
        url: gh.html_url,
        asset,
    })
}

pub async fn check(client: &reqwest::Client) -> Result<Option<Release>, String> {
    let resp = client
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub ({e})"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("GitHub is rate limiting update checks, try again later".into());
    }
    if !status.is_success() {
        return Err(format!("GitHub answered {status}"));
    }
    let gh: GhRelease = resp.json().await.map_err(|e| format!("unexpected answer from GitHub ({e})"))?;
    Ok(release_from(gh, target()).filter(|r| is_newer(&r.version, CURRENT)))
}

pub fn install_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("could not find the running binary ({e})"))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    if is_dev_build(&exe) {
        return Err("this is a development build, update it with git pull and cargo build".into());
    }
    Ok(exe)
}

fn is_dev_build(exe: &Path) -> bool {
    exe.ancestors().skip(1).any(|p| {
        matches!(p.file_name().and_then(|n| n.to_str()), Some("debug" | "release"))
            && p.parent().and_then(|q| q.file_name()).is_some_and(|n| n == "target")
    })
}

fn run_version(path: &Path) -> std::io::Result<std::process::Output> {
    let mut tries = 0;
    loop {
        let out = std::process::Command::new(path)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        match out {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

pub async fn download<F>(client: &reqwest::Client, asset: &Asset, mut progress: F) -> Result<Vec<u8>, String>
where
    F: FnMut(u64, u64),
{
    let mut resp = client
        .get(&asset.url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("could not download {} ({e})", asset.name))?;
    if !resp.status().is_success() {
        return Err(format!("downloading {} failed with {}", asset.name, resp.status()));
    }
    let total = resp.content_length().filter(|n| *n > 0).unwrap_or(asset.size);
    if total > MAX_BINARY {
        return Err("the download is unexpectedly large".into());
    }
    let mut data = Vec::with_capacity(total as usize);
    progress(0, total);
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("the download broke off ({e})"))? {
        data.extend_from_slice(&chunk);
        if data.len() as u64 > MAX_BINARY {
            return Err("the download is unexpectedly large".into());
        }
        progress(data.len() as u64, total);
    }
    if asset.size > 0 && data.len() as u64 != asset.size {
        return Err(format!("the download is incomplete ({} of {} bytes)", data.len(), asset.size));
    }
    Ok(data)
}

pub fn replace(dest: &Path, data: &[u8], version: &str) -> Result<(), String> {
    let dir = dest.parent().ok_or("the binary has no folder")?;
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("lyrminal");
    let tmp = dir.join(format!(".{name}.update-{}", std::process::id()));
    let result = write_and_swap(&tmp, dest, data, version);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_and_swap(tmp: &Path, dest: &Path, data: &[u8], version: &str) -> Result<(), String> {
    let dir = dest.parent().unwrap_or(Path::new("."));
    std::fs::write(tmp, data).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            format!("no permission to write to {}, update with the tool that installed Lyrminal", dir.display())
        }
        _ => format!("could not write to {} ({e})", dir.display()),
    })?;
    set_executable(tmp)?;
    let out = run_version(tmp).map_err(|e| format!("the downloaded binary does not run ({e})"))?;
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let want = format!("lyrminal {version}");
    if got != want {
        return Err(format!("the downloaded binary reports “{got}”, expected “{want}”"));
    }
    std::fs::rename(tmp, dest).map_err(|e| format!("could not replace {} ({e})", dest.display()))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("could not make the binary executable ({e})"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub async fn install<F>(client: &reqwest::Client, release: &Release, progress: F) -> Result<PathBuf, String>
where
    F: FnMut(u64, u64),
{
    let asset = release.asset.as_ref().ok_or("there is no prebuilt binary for this system")?;
    let dest = install_path()?;
    let data = download(client, asset, progress).await?;
    let version = release.version.clone();
    let path = dest.clone();
    tokio::task::spawn_blocking(move || replace(&path, &data, &version))
        .await
        .map_err(|e| e.to_string())??;
    Ok(dest)
}

fn restart_args() -> Vec<std::ffi::OsString> {
    crate::config::relaunch_args()
}

#[cfg(unix)]
pub fn restart(path: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(path).args(restart_args()).exec()
}

#[cfg(not(unix))]
pub fn restart(path: &Path) -> std::io::Error {
    match std::process::Command::new(path).args(restart_args()).spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(is_newer("1.1.0", "1.0.9"));
        assert!(is_newer("v2.0.0", "1.12.30"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("v0.12.0", "1.0.0"));
        assert!(!is_newer("nightly", "1.0.0"));
        assert_eq!(parse_version("v1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn reads_a_github_release() {
        let json = r###"{
            "tag_name": "v1.0.1",
            "html_url": "https://github.com/BajekekButLost/lyrminal/releases/tag/v1.0.1",
            "body": "## Notes\n\nLyrminal 1.0.1: updates from inside\nthe app.\n\n## Features\n\n- stuff",
            "assets": [
                {"name": "lyrminal-v1.0.1-aarch64-linux", "browser_download_url": "https://example.com/arm", "size": 5},
                {"name": "lyrminal-v1.0.1-x86_64-linux", "browser_download_url": "https://example.com/x86", "size": 7}
            ]
        }"###;
        let gh: GhRelease = serde_json::from_str(json).unwrap();
        let r = release_from(gh, Some("x86_64-linux")).unwrap();
        assert_eq!(r.version, "1.0.1");
        assert_eq!(r.summary, "Lyrminal 1.0.1: updates from inside the app.");
        assert_eq!(r.asset.as_ref().map(|a| (a.url.as_str(), a.size)), Some(("https://example.com/x86", 7)));
        let gh: GhRelease = serde_json::from_str(json).unwrap();
        assert!(release_from(gh, Some("riscv64-linux")).unwrap().asset.is_none());
        let gh: GhRelease = serde_json::from_str(&json.replace("\"v1.0.1\"", "\"v1.0.1\", \"prerelease\": true")).unwrap();
        assert!(release_from(gh, Some("x86_64-linux")).is_none());
    }

    #[test]
    fn spots_development_builds() {
        assert!(is_dev_build(Path::new("/home/me/lyrminal/target/release/lyrminal")));
        assert!(is_dev_build(Path::new("/home/me/lyrminal/target/debug/lyrminal")));
        assert!(is_dev_build(Path::new("/home/me/lyrminal/target/debug/deps/lyrminal-1234")));
        assert!(!is_dev_build(Path::new("/home/me/.local/bin/lyrminal")));
        assert!(!is_dev_build(Path::new("/opt/release/lyrminal")));
    }

    #[tokio::test]
    #[ignore]
    async fn live_check_and_install() {
        let client = crate::providers::http_client();
        let newer = check(&client).await.unwrap();
        assert!(newer.as_ref().is_none_or(|r| is_newer(&r.version, CURRENT)));
        let tag = "v1.0.0";
        let asset = Asset {
            name: asset_name(tag, "x86_64-linux"),
            url: format!("https://github.com/{REPO}/releases/download/{tag}/{}", asset_name(tag, "x86_64-linux")),
            size: 0,
        };
        let mut seen = 0;
        let data = download(&client, &asset, |done, _| seen = done).await.unwrap();
        assert_eq!(seen, data.len() as u64);
        let dir = std::env::temp_dir().join(format!("lyrminal-live-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("lyrminal");
        std::fs::write(&dest, "old").unwrap();
        replace(&dest, &data, "1.0.0").unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), data.len() as u64);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn swaps_in_a_verified_binary() {
        let dir = std::env::temp_dir().join(format!("lyrminal-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("lyrminal");
        std::fs::write(&dest, "old").unwrap();
        let good = b"#!/bin/sh\necho 'lyrminal 9.9.9'\n";
        let bad = b"#!/bin/sh\necho 'lyrminal 0.0.1'\n";
        let err = replace(&dest, bad, "9.9.9").unwrap_err();
        assert!(err.contains("expected “lyrminal 9.9.9”"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        replace(&dest, good, "9.9.9").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), good);
        let _ = std::fs::remove_dir_all(dir);
    }
}
