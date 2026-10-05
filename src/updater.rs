//! In-app updater: finds the latest GitHub release, downloads it, verifies its
//! SHA-256 and swaps the running `.app` bundle for the new one (macOS only).
//!
//! The network and bundle work shells out to tools that ship with macOS
//! (`curl`, `shasum`, `ditto`, `codesign`, `osascript`), so no extra HTTP/TLS
//! stack is linked in. Everything here is blocking; callers run it through
//! `ui::runtime::spawn_blocking`.
//!
//! Flow: [`check`] -> [`download`] (checksum verified) -> [`install`] ->
//! [`relaunch`]. When the install folder is not writable by the user (an
//! admin-owned `/Applications`) the swap runs through the standard macOS
//! administrator password prompt.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const REPO: &str = "tesmond/multidb";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CURL: &str = if cfg!(target_os = "macos") { "/usr/bin/curl" } else { "curl" };

/// Returned (inside the `anyhow::Error`) when the user cancels a download.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("update cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Where users can grab a release by hand (offered when an update fails).
pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// A newer release this build can install.
#[derive(Debug, Clone)]
pub struct Release {
    /// Version without the leading `v` (`0.5.0`).
    pub version: String,
    /// Release notes (markdown, as written on GitHub).
    pub notes: String,
    pub page_url: String,
    pub zip_name: String,
    pub zip_url: String,
    pub zip_size: u64,
    pub sha_url: Option<String>,
}

// ─── Versions ───────────────────────────────────────────────────────────────

/// `0.4.0`, `v1.2` or `1.2.3-beta.1` -> `(major, minor, patch)`.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    Some((major, minor, patch))
}

fn arch_tag() -> &'static str {
    // Matches packaging/macos/bundle.sh.
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "arm64"
    }
}

/// Decide whether `gh` is an upgrade over `current` and which assets to use.
fn pick_release(gh: GhRelease, current: &str, arch: &str) -> Result<Option<Release>> {
    let latest = parse_version(&gh.tag_name).ok_or_else(|| anyhow!("unrecognised release version {:?}", gh.tag_name))?;
    let have = parse_version(current).ok_or_else(|| anyhow!("unrecognised app version {current:?}"))?;
    if latest <= have {
        return Ok(None); // up to date, and never offer a downgrade
    }
    let version = gh.tag_name.trim_start_matches('v').to_string();
    let suffix = format!("-macos-{arch}.zip");
    let zip = gh
        .assets
        .iter()
        .find(|a| a.name.starts_with("MultiDB-") && a.name.ends_with(&suffix))
        .ok_or_else(|| anyhow!("release {version} has no macOS ({arch}) download"))?;
    let sha_name = format!("{}.sha256", zip.name);
    let sha_url = gh.assets.iter().find(|a| a.name == sha_name).map(|a| a.browser_download_url.clone());
    Ok(Some(Release {
        version,
        notes: gh.body.unwrap_or_default(),
        page_url: gh.html_url,
        zip_name: zip.name.clone(),
        zip_url: zip.browser_download_url.clone(),
        zip_size: zip.size,
        sha_url,
    }))
}

// ─── Check ──────────────────────────────────────────────────────────────────

/// Why updating is impossible here, or `None` when this build can update itself.
pub fn unavailable_reason() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return Some("Automatic updates are only supported on macOS for now. Download the latest release from GitHub.".into());
    }
    if app_bundle().is_none() {
        return Some("Updates are unavailable in development builds: multidb is not running from a .app bundle.".into());
    }
    None
}

/// Ask GitHub for the latest published (non-draft, non-prerelease) release.
/// `Ok(None)` means this build is already current.
pub fn check() -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = Command::new(CURL)
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-H"])
        .arg(format!("User-Agent: multidb-updater/{CURRENT_VERSION}"))
        .arg(&url)
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        bail!("could not reach GitHub: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let gh: GhRelease = serde_json::from_slice(&out.stdout).context("unexpected response from GitHub")?;
    pick_release(gh, CURRENT_VERSION, arch_tag())
}

// ─── Download ───────────────────────────────────────────────────────────────

/// Scratch folder for one update attempt.
pub fn staging_dir() -> PathBuf {
    std::env::temp_dir().join(format!("multidb-update-{}", std::process::id()))
}

/// File the zip is streamed into; its size drives the progress bar.
pub fn part_path(dir: &Path, rel: &Release) -> PathBuf {
    dir.join(format!("{}.part", rel.zip_name))
}

fn parse_sha256(text: &str) -> Result<String> {
    let token = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(token)
    } else {
        bail!("the release's checksum file is malformed")
    }
}

fn sha256_of(path: &Path) -> Result<String> {
    let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(path).output().context("could not run shasum")?;
    if !out.status.success() {
        bail!("shasum failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_sha256(&String::from_utf8_lossy(&out.stdout))
}

fn curl_to_stdout(url: &str) -> Result<String> {
    let out = Command::new(CURL)
        .args(["-fsSL", "--max-time", "30", "--retry", "2"])
        .arg(url)
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        bail!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Download the zip into `dir` and verify it against the release's `.sha256`.
/// Refuses to continue when the release publishes no checksum.
pub fn download(rel: &Release, dir: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
    let sha_url = rel
        .sha_url
        .as_deref()
        .ok_or_else(|| anyhow!("this release has no checksum file, so it can't be verified. Download it manually instead"))?;
    std::fs::create_dir_all(dir).context("could not create a temporary folder")?;
    let expected = parse_sha256(&curl_to_stdout(sha_url)?)?;

    let part = part_path(dir, rel);
    let _ = std::fs::remove_file(&part);
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }
    let mut child = Command::new(CURL)
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "20", "-o"])
        .arg(&part)
        .arg(&rel.zip_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run curl")?;
    // Poll so a cancel request can kill the transfer promptly.
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&part);
            return Err(Cancelled.into());
        }
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output().context("could not run curl")?;
    if !out.status.success() {
        bail!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let len = std::fs::metadata(&part)?.len();
    if rel.zip_size != 0 && len != rel.zip_size {
        bail!("download was incomplete ({len} of {} bytes)", rel.zip_size);
    }
    if sha256_of(&part)? != expected {
        let _ = std::fs::remove_file(&part);
        bail!("checksum mismatch: the download is corrupt or has been tampered with");
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&part);
        return Err(Cancelled.into());
    }
    let zip = dir.join(&rel.zip_name);
    std::fs::rename(&part, &zip)?;
    Ok(zip)
}

// ─── Install ────────────────────────────────────────────────────────────────

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn applescript_quote(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

/// Swap `$2` (the installed app) for a copy of `$1`, rolling back if the final
/// rename fails. Works on the same volume as the target so the rename is atomic.
/// Run as the user, or as root through the macOS admin prompt.
const SWAP_SCRIPT: &str = r#"#!/bin/sh
# usage: swap.sh NEW_APP TARGET_APP OWNER(uid:gid)
set -eu
NEW="$1"; TARGET="$2"; OWNER="$3"
STAGE="$TARGET.update-new"
OLD="$TARGET.update-old"
trap 'rm -rf "$STAGE"' EXIT
rm -rf "$STAGE" "$OLD"
/usr/bin/ditto "$NEW" "$STAGE"
/usr/sbin/chown -R "$OWNER" "$STAGE"
/usr/bin/xattr -dr com.apple.quarantine "$STAGE" 2>/dev/null || true
mv "$TARGET" "$OLD"
if mv "$STAGE" "$TARGET"; then
  rm -rf "$OLD" || true
else
  mv "$OLD" "$TARGET"
  exit 1
fi
"#;

fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("could not {what}"))?;
    if !out.status.success() {
        bail!("could not {what}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    /// The `.app` this process runs from, or `None` for `cargo run` / a bare binary.
    pub fn app_bundle() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
        let macos_dir = exe.parent()?;
        let contents = macos_dir.parent()?;
        let bundle = contents.parent()?;
        let is_bundle = macos_dir.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app";
        is_bundle.then(|| bundle.to_path_buf())
    }

    /// True when the user can't create files next to the bundle, so the swap
    /// needs administrator rights.
    pub fn needs_admin(bundle: &Path) -> bool {
        let Some(parent) = bundle.parent() else { return true };
        let probe = parent.join(format!(".multidb-write-test-{}", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                false
            }
            Err(_) => true,
        }
    }

    /// Unpack `zip`, sanity-check the bundle and swap it in for the running one.
    /// Returns the installed bundle's path.
    pub fn install(zip: &Path, work: &Path) -> Result<PathBuf> {
        let bundle = app_bundle().ok_or_else(|| anyhow!("multidb is not running from a .app bundle"))?;

        let extract = work.join("extracted");
        let _ = std::fs::remove_dir_all(&extract);
        std::fs::create_dir_all(&extract)?;
        run(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(zip).arg(&extract), "unpack the update")?;

        let new_app = std::fs::read_dir(&extract)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|x| x == "app"))
            .ok_or_else(|| anyhow!("the download doesn't contain an app"))?;
        if !new_app.join("Contents/MacOS/multidb").is_file() {
            bail!("the downloaded app is incomplete");
        }
        run(
            Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&new_app),
            "verify the downloaded app's signature",
        )?;

        let meta = std::fs::metadata(&bundle)?;
        let owner = format!("{}:{}", meta.uid(), meta.gid());
        let script = work.join("swap.sh");
        std::fs::write(&script, SWAP_SCRIPT)?;
        let (script_s, new_s, bundle_s) = (script.to_string_lossy(), new_app.to_string_lossy(), bundle.to_string_lossy());

        if needs_admin(&bundle) {
            let cmd = format!("/bin/sh {} {} {} {}", sh_quote(&script_s), sh_quote(&new_s), sh_quote(&bundle_s), sh_quote(&owner));
            let folder = bundle.parent().map(|p| p.display().to_string()).unwrap_or_default();
            let osa = format!(
                "do shell script \"{}\" with administrator privileges with prompt \"{}\"",
                applescript_quote(&cmd),
                applescript_quote(&format!("multidb needs your permission to update itself in {folder}."))
            );
            let out = Command::new("/usr/bin/osascript").arg("-e").arg(osa).stdin(Stdio::null()).output().context("could not ask for permission")?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                if err.contains("-128") || err.to_lowercase().contains("cancel") {
                    bail!("update cancelled: administrator permission was not granted");
                }
                bail!("could not install the update: {}", err.trim());
            }
        } else {
            run(Command::new("/bin/sh").arg(&script).arg(&new_app).arg(&bundle).arg(&owner), "install the update")?;
        }
        Ok(bundle)
    }

    /// Start the new app once this process has exited, and clean up `work`.
    /// The caller quits right after.
    pub fn relaunch(bundle: &Path, work: &Path) {
        let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; rm -rf "$3"; exec /usr/bin/open -n "$2""#;
        let _ = Command::new("/bin/sh")
            .args(["-c", script, "sh", &std::process::id().to_string()])
            .arg(bundle)
            .arg(work)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn app_bundle() -> Option<PathBuf> {
        None
    }

    #[allow(dead_code)]
    pub fn install(_zip: &Path, _work: &Path) -> Result<PathBuf> {
        bail!("automatic updates are only supported on macOS")
    }

    #[allow(dead_code)]
    pub fn relaunch(_bundle: &Path, _work: &Path) {}
}

pub use imp::{app_bundle, install, relaunch};

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(tag: &str, assets: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            body: Some("notes".into()),
            html_url: "https://github.com/x/y/releases/tag/t".into(),
            assets: assets
                .iter()
                .map(|n| GhAsset { name: (*n).into(), size: 10, browser_download_url: format!("https://dl/{n}") })
                .collect(),
        }
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn compares_numerically_not_lexically() {
        assert!(parse_version("0.10.0") > parse_version("0.9.0"));
        assert!(parse_version("1.0.0") > parse_version("0.99.99"));
    }

    #[test]
    fn offers_newer_release_with_checksum() {
        let assets = ["MultiDB-0.5.0-macos-arm64.zip", "MultiDB-0.5.0-macos-arm64.zip.sha256", "MultiDB-0.5.0-windows-x64.zip"];
        let rel = pick_release(gh("v0.5.0", &assets), "0.4.0", "arm64").unwrap().unwrap();
        assert_eq!(rel.version, "0.5.0");
        assert_eq!(rel.zip_name, "MultiDB-0.5.0-macos-arm64.zip");
        assert_eq!(rel.sha_url.as_deref(), Some("https://dl/MultiDB-0.5.0-macos-arm64.zip.sha256"));
    }

    #[test]
    fn same_or_older_release_is_not_offered() {
        let assets = ["MultiDB-0.4.0-macos-arm64.zip"];
        assert!(pick_release(gh("v0.4.0", &assets), "0.4.0", "arm64").unwrap().is_none());
        assert!(pick_release(gh("v0.3.0", &assets), "0.4.0", "arm64").unwrap().is_none());
    }

    #[test]
    fn missing_platform_asset_is_an_error() {
        let assets = ["MultiDB-0.5.0-windows-x64.zip"];
        assert!(pick_release(gh("v0.5.0", &assets), "0.4.0", "arm64").is_err());
    }

    #[test]
    fn release_without_checksum_has_no_sha_url() {
        let assets = ["MultiDB-0.5.0-macos-arm64.zip"];
        let rel = pick_release(gh("v0.5.0", &assets), "0.4.0", "arm64").unwrap().unwrap();
        assert!(rel.sha_url.is_none());
    }

    #[test]
    fn parses_sha256_files() {
        let h = "a".repeat(64);
        assert_eq!(parse_sha256(&format!("{h}  MultiDB.zip\n")).unwrap(), h);
        assert_eq!(parse_sha256(&h.to_uppercase()).unwrap(), h);
        assert!(parse_sha256("deadbeef  file").is_err());
        assert!(parse_sha256("").is_err());
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(applescript_quote(r#"a"b\c"#), r#"a\"b\\c"#);
    }
}
