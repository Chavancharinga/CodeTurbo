//! Update notices and self-update from GitHub Releases. The check runs at
//! most once a day, in the background while the intro plays, and never
//! blocks or fails a session.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "Chavancharinga/CodeTurbo";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long a check result is trusted before asking GitHub again.
const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;
const CACHE_FILE: &str = "update-check.txt";

/// `v1.2.3` or `1.2.3` as `(1, 2, 3)`.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

/// Whether release tag `latest` is newer than version `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// Release asset for an OS (`std::env::consts::OS`) and CPU architecture.
pub fn asset_name_for(os: &str, arch: &str) -> Option<String> {
    let target = match (os, arch) {
        ("windows", "x86_64" | "aarch64") => "x86_64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        // Termux: a build for Android itself, which only runs PIE programs.
        ("android", "aarch64") => "aarch64-linux-android",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => return None,
    };
    Some(format!("codeturbo-{target}.tar.gz"))
}

pub fn asset_name() -> Option<String> {
    asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn client() -> Result<reqwest::Client, String> {
    coturbo_providers::dns::client_builder()
        .user_agent(concat!("codeturbo/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Tag of the latest GitHub release.
pub async fn latest_tag() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = client()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("o GitHub respondeu {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    v["tag_name"].as_str().map(str::to_string).ok_or_else(|| "the published release has no tag".into())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Cached `(checked_at, latest_tag)`.
fn read_cache(dir: &Path) -> Option<(u64, String)> {
    let text = std::fs::read_to_string(dir.join(CACHE_FILE)).ok()?;
    let (at, tag) = text.trim().split_once(' ')?;
    Some((at.parse().ok()?, tag.to_string()))
}

fn write_cache(dir: &Path, at: u64, tag: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(CACHE_FILE), format!("{at} {tag}\n"));
}

/// The newer release tag, if any: from the cache when it is fresh,
/// otherwise from GitHub (and cached). Errors are treated as "no news".
pub async fn check(cache_dir: Option<PathBuf>, now: u64) -> Option<String> {
    let cached = cache_dir.as_deref().and_then(read_cache);
    let tag = match cached {
        Some((at, tag)) if now.saturating_sub(at) < CHECK_EVERY_SECS => tag,
        _ => {
            let tag = tokio::time::timeout(Duration::from_secs(3), latest_tag()).await.ok()?.ok()?;
            if let Some(dir) = &cache_dir {
                write_cache(dir, now, &tag);
            }
            tag
        }
    };
    is_newer(&tag, VERSION).then_some(tag)
}

/// [`check`] at the current time.
pub async fn check_now(cache_dir: Option<PathBuf>) -> Option<String> {
    check(cache_dir, now_secs()).await
}

pub fn notice(tag: &str) -> String {
    format!("A new CodeTurbo version is available: {tag} (installed: v{VERSION}). To update, run `codeturbo update`.")
}

/// The `codeturbo` executable inside a release `.tar.gz`.
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, String> {
    let exe = if cfg!(windows) { "codeturbo.exe" } else { "codeturbo" };
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path.file_name().is_some_and(|n| n == exe) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            return Ok(bytes);
        }
    }
    Err(format!("{exe} not found in the release package"))
}

/// Where the previous binary is parked during an update.
fn parked(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Replaces the executable at `exe` with `bytes`. A running program can't
/// be overwritten on Windows, but it can be renamed: the old binary is
/// moved aside and removed on the next start.
pub fn replace_binary(exe: &Path, bytes: &[u8]) -> Result<(), String> {
    let fresh = exe.with_extension("new");
    std::fs::write(&fresh, bytes).map_err(|e| format!("{}: {e}", fresh.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o755));
    }
    let mut old = parked(exe);
    // A program still open from an earlier update keeps its parked file in
    // use, and Windows will not delete it: park this one under a new name.
    if std::fs::remove_file(&old).is_err() && old.exists() {
        let mut name = old.file_name().unwrap_or_default().to_os_string();
        name.push(format!("-{}", std::process::id()));
        old = old.with_file_name(name);
    }
    std::fs::rename(exe, &old).map_err(|e| format!("{}: {e}", exe.display()))?;
    if let Err(e) = std::fs::rename(&fresh, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("{}: {e}", exe.display()));
    }
    Ok(())
}

/// Removes the binaries left over by previous updates (those still running
/// stay until a later start).
pub fn cleanup_old_binary() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let parked_name = parked(&exe).file_name().unwrap_or_default().to_string_lossy().into_owned();
    let fresh = exe.with_extension("new");
    let _ = std::fs::remove_file(fresh);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&parked_name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `codeturbo update`: downloads the latest release for this platform and
/// replaces the running executable.
pub async fn self_update() -> Result<String, String> {
    let tag = latest_tag().await?;
    if !is_newer(&tag, VERSION) {
        return Ok(format!("CodeTurbo is already up to date (v{VERSION})."));
    }
    let asset = asset_name().ok_or("there is no ready executable for this platform; update with cargo install")?;
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{asset}");
    let resp = client()?.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{url}: {}", resp.status()));
    }
    let archive = resp.bytes().await.map_err(|e| e.to_string())?;
    let bytes = extract_binary(&archive)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_binary(&exe, &bytes)?;
    Ok(format!("CodeTurbo updated: v{VERSION} -> {tag}."))
}
