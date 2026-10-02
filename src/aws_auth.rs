//! AWS IAM authentication for RDS MySQL connections.
//!
//! An RDS IAM auth token is signed with the caller's AWS credentials and is
//! only good for fifteen minutes, so one is generated for every connect and
//! reconnect. Signing needs live credentials, and on the SSO profiles most
//! people use those come from a cached access token that expires — typically
//! once a day, and always at the least convenient moment.
//!
//! Left alone, that surfaces as a connection failure telling the user to go and
//! run `aws sso login` themselves. This module does it for them: it checks the
//! cached SSO session before signing, and signs in again if the session has
//! expired or was never there. If a token still fails to sign for a
//! credentials-shaped reason, it signs in once more and retries, because a
//! session can lapse between the check and the call. A connection only fails
//! when signing in fails.

use crate::ipc_diagnostics;
use crate::models::ConnectionConfig;
use anyhow::{anyhow, Context, Result};
use aws_config::{BehaviorVersion, Region};
use aws_sdk_rds::auth_token::{AuthTokenGenerator, Config as AuthTokenConfig};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const SDK_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// `aws sso login` opens a browser and waits for the person to approve there,
/// so this is a human timeout rather than a network one.
const SSO_LOGIN_TIMEOUT: Duration = Duration::from_secs(180);
/// Treat a session that is about to lapse as already lapsed, so a token is
/// never signed with credentials that die in the middle of connecting.
const SSO_EXPIRY_MARGIN: chrono::TimeDelta = chrono::TimeDelta::minutes(2);
/// How long an RDS auth token is valid for. `AWS_IAM_REFRESH_AGE` in
/// `connections` reconnects before this runs out.
pub const TOKEN_LIFETIME_SECS: u64 = 900;

/// Generate an RDS IAM auth token for `cfg`, signing in to AWS first if that is
/// what the credentials need.
pub async fn auth_token(cfg: &ConnectionConfig) -> Result<String> {
    let profile = AwsProfile::resolve(cfg);

    // The common case for an SSO profile: yesterday's session has lapsed. Renew
    // it before signing rather than after failing to. Best-effort, though — the
    // SDK looks at environment variables and instance roles before it looks at
    // the profile, so a stale SSO session is not proof that the connection is
    // going to fail, and a failure to sign in must not become one.
    let mut login_failure = None;
    if profile.sso_session_lapsed() {
        login_failure = ensure_signed_in(&profile).await.err();
    }

    let first = match generate_token(cfg, &profile).await {
        Ok(token) => return Ok(token),
        Err(err) => err,
    };

    // Signing in cannot help a profile that has nothing to sign in to, or an
    // error that is not about credentials: report those as they are.
    if !profile.is_sso() || !looks_like_credentials_problem(&first) {
        return Err(first);
    }
    if let Some(failure) = login_failure {
        // Already tried before signing, so there is nothing new to attempt.
        return Err(first.context(format!("signing in to AWS SSO also failed: {failure}")));
    }
    // The session was live at the check and is not now, or it was never the
    // problem until the SDK said so. Either way, one sign-in and one retry.
    ensure_signed_in(&profile)
        .await
        .with_context(|| format!("AWS credentials for profile {} were rejected: {first}", profile.name))?;
    generate_token(cfg, &profile).await
}

async fn generate_token(cfg: &ConnectionConfig, profile: &AwsProfile) -> Result<String> {
    ipc_diagnostics::set_test_connection_stage("aws_sdk_load_config");
    let mut loader =
        aws_config::defaults(BehaviorVersion::latest()).region(Region::new(cfg.aws_region.trim().to_string()));
    if !cfg.aws_profile.trim().is_empty() {
        loader = loader.profile_name(cfg.aws_profile.trim().to_string());
    }

    let sdk_config = crate::connections::timeout_step(
        SDK_LOAD_TIMEOUT,
        format!("load AWS SDK config for region {} with profile {}", cfg.aws_region.trim(), profile.name),
        loader.load(),
    )
    .await?;

    ipc_diagnostics::set_test_connection_stage("aws_generate_iam_token");
    let token_config = AuthTokenConfig::builder()
        .hostname(cfg.host.trim())
        .port(cfg.port as u64)
        .username(cfg.username.trim())
        .expires_in(TOKEN_LIFETIME_SECS)
        .build()
        .map_err(|err| anyhow!("build AWS RDS auth token config: {err}"))?;

    let token = crate::connections::timeout_step(
        TOKEN_TIMEOUT,
        format!(
            "generate AWS RDS IAM auth token for {}@{}:{}",
            cfg.username.trim(),
            cfg.host.trim(),
            cfg.port
        ),
        AuthTokenGenerator::new(token_config).auth_token(&sdk_config),
    )
    .await?
    .map_err(|err| anyhow!("generate AWS RDS auth token: {err}"))?;

    Ok(token.to_string())
}

// ─── Signing in ─────────────────────────────────────────────────────────────

/// One `aws sso login` per profile at a time. Opening several connections at
/// once — which is exactly what happens when the app restores a session — would
/// otherwise spawn a browser window each.
fn login_lock(profile: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().expect("aws login locks");
    Arc::clone(locks.entry(profile.to_string()).or_default())
}

/// Make sure the profile has a usable SSO session, signing in if it does not.
/// Idempotent: whoever gets the lock second usually finds the session already
/// renewed and does nothing.
async fn ensure_signed_in(profile: &AwsProfile) -> Result<()> {
    let Some(start_url) = profile.sso_start_url.as_deref() else {
        return Err(anyhow!(
            "AWS profile {} is not configured for SSO, so its credentials cannot be renewed automatically. \
             Set up the profile in ~/.aws/config, or refresh its credentials yourself.",
            profile.name
        ));
    };
    let lock = login_lock(&profile.name);
    let _held = lock.lock().await;
    if !profile.sso_session_lapsed() {
        return Ok(());
    }

    ipc_diagnostics::set_test_connection_stage("aws_sso_login");
    let program = which_aws().ok_or_else(|| {
        anyhow!(
            "the AWS CLI is needed to sign in to {start_url} for profile {} but was not found on PATH. \
             Install it, or sign in yourself with `aws sso login --profile {}`.",
            profile.name,
            profile.name
        )
    })?;

    let output = crate::connections::timeout_step(
        SSO_LOGIN_TIMEOUT,
        format!("sign in to AWS SSO for profile {} (a browser window needs approving)", profile.name),
        tokio::process::Command::new(&program)
            .args(sso_login_args(&profile.name))
            .env("PATH", child_path(Some(&program)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await?
    .with_context(|| format!("run {} sso login", program.display()))?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(anyhow!(
            "`aws sso login --profile {}` failed{}{}",
            profile.name,
            if detail.is_empty() { "" } else { ": " },
            detail
        ));
    }
    Ok(())
}

fn sso_login_args(profile: &str) -> Vec<String> {
    vec!["sso".to_string(), "login".to_string(), "--profile".to_string(), profile.to_string()]
}

fn which_aws() -> Option<PathBuf> {
    let path = child_path(None);
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) { "aws.exe" } else { "aws" });
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn child_path(program: Option<&Path>) -> OsString {
    crate::connections::external_tool_path(program)
}

// ─── Reading the profile and the SSO cache ──────────────────────────────────

/// What this needs to know about the AWS profile a connection uses: its name,
/// and the SSO start URL when it signs in through SSO. The start URL is the key
/// to the cached session, and its absence is what says "this profile cannot be
/// renewed by signing in".
#[derive(Debug, Clone, PartialEq)]
pub struct AwsProfile {
    pub name: String,
    pub sso_start_url: Option<String>,
}

impl AwsProfile {
    fn resolve(cfg: &ConnectionConfig) -> Self {
        let name = profile_name(cfg.aws_profile.trim(), std::env::var("AWS_PROFILE").ok().as_deref());
        let config = config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        AwsProfile { sso_start_url: sso_start_url(&config, &name), name }
    }

    fn is_sso(&self) -> bool {
        self.sso_start_url.is_some()
    }

    /// True when this is an SSO profile whose cached session is missing, about
    /// to expire, or already expired — i.e. when signing in is worth doing.
    fn sso_session_lapsed(&self) -> bool {
        let Some(start_url) = self.sso_start_url.as_deref() else {
            return false;
        };
        let Some(dir) = sso_cache_dir() else {
            return true;
        };
        !cached_session_is_live(&dir, start_url, chrono::Utc::now())
    }
}

fn profile_name(configured: &str, env_profile: Option<&str>) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    match env_profile.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => "default".to_string(),
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AWS_CONFIG_FILE") {
        return Some(PathBuf::from(path));
    }
    dirs::home_dir().map(|home| home.join(".aws").join("config"))
}

fn sso_cache_dir() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".aws").join("sso").join("cache");
    dir.is_dir().then_some(dir)
}

/// The SSO start URL a profile signs in to, from the text of `~/.aws/config`.
///
/// Profiles reach it either directly (`sso_start_url`, the original form) or
/// through a shared `sso_session` block, so both are followed.
fn sso_start_url(config: &str, profile: &str) -> Option<String> {
    let section = if profile == "default" { "default".to_string() } else { format!("profile {profile}") };
    let entries = ini_section(config, &section)?;
    if let Some(url) = entries.get("sso_start_url") {
        return Some(url.clone());
    }
    let session = entries.get("sso_session")?;
    ini_section(config, &format!("sso-session {session}"))?.get("sso_start_url").cloned()
}

/// The key/value pairs of one `[section]` of an AWS config file. Enough of INI
/// for this purpose: `key = value` lines, `#`/`;` comments, and the indented
/// continuation lines AWS uses for nested settings are ignored rather than
/// parsed, because nothing here lives inside one.
fn ini_section(text: &str, wanted: &str) -> Option<HashMap<String, String>> {
    let mut entries = HashMap::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if inside {
                return Some(entries);
            }
            // Section names are whitespace-normalised: `[profile  foo]` names
            // the same profile as `[profile foo]`.
            inside = name.split_whitespace().collect::<Vec<_>>() == wanted.split_whitespace().collect::<Vec<_>>();
            continue;
        }
        if inside {
            if let Some((key, value)) = line.split_once('=') {
                entries.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    inside.then_some(entries)
}

/// Whether the SSO cache holds a session for `start_url` that is still good.
///
/// The cache is a directory of JSON blobs keyed by a hash the CLI computes, and
/// it also holds client registrations that have no `startUrl` at all. Rather
/// than reproduce the hashing, read them all and look for one that matches.
fn cached_session_is_live(dir: &Path, start_url: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if session_expiry(&text, start_url).is_some_and(|expires| expires > now + SSO_EXPIRY_MARGIN) {
            return true;
        }
    }
    false
}

/// When the cached session in `text` expires, if it is for `start_url` at all.
fn session_expiry(text: &str, start_url: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let cached = value.get("startUrl")?.as_str()?;
    if cached.trim_end_matches('/') != start_url.trim_end_matches('/') {
        return None;
    }
    value.get("accessToken")?.as_str()?;
    let expires = value.get("expiresAt")?.as_str()?;
    parse_expiry(expires)
}

/// `expiresAt` is RFC 3339, but the CLI has also written a trailing-`UTC` form
/// (`2026-09-18T07:21:33UTC`) that the standard parsers reject.
fn parse_expiry(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let value = value.trim();
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&chrono::Utc));
    }
    let naive = value.strip_suffix("UTC").unwrap_or(value).trim();
    chrono::NaiveDateTime::parse_from_str(naive, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Whether an error from the SDK is the kind that signing in again would fix.
///
/// The SDK reports these as prose rather than as a type the caller can match
/// on, so this reads the whole chain for the words that only appear when
/// credentials are missing, expired or refused.
fn looks_like_credentials_problem(err: &anyhow::Error) -> bool {
    const MARKERS: &[&str] = &[
        "sso session",
        "sso",
        "expiredtoken",
        "token has expired",
        "token is expired",
        "credentials",
        "credential",
        "invalidgrant",
        "invalid_grant",
        "accessdenied",
        "access denied",
        "unauthorized",
        "not authorized",
        "forbidden",
    ];
    let text = error_chain(err);
    MARKERS.iter().any(|marker| text.contains(marker))
}

fn error_chain(err: &anyhow::Error) -> String {
    err.chain().map(|cause| cause.to_string()).collect::<Vec<_>>().join("; ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
[default]
region = eu-west-1

[profile legacy-sso]
sso_start_url = https://acme.awsapps.com/start
sso_account_id = 123456789012

[profile modern]
sso_session = acme
region = eu-west-2

[profile keys]
aws_access_key_id = AKIA0000

[sso-session acme]
sso_start_url = https://acme.awsapps.com/start#
sso_region = eu-west-1
"#;

    #[test]
    fn the_profile_is_the_configured_one_then_the_environment_then_default() {
        assert_eq!(profile_name("ops", Some("env")), "ops");
        assert_eq!(profile_name("", Some("env")), "env");
        assert_eq!(profile_name("", Some("  ")), "default");
        assert_eq!(profile_name("", None), "default");
    }

    #[test]
    fn the_start_url_is_found_directly_and_through_an_sso_session() {
        assert_eq!(sso_start_url(CONFIG, "legacy-sso").as_deref(), Some("https://acme.awsapps.com/start"));
        assert_eq!(sso_start_url(CONFIG, "modern").as_deref(), Some("https://acme.awsapps.com/start#"));
        // A profile with static keys has nothing to sign in to, and neither
        // does a profile that is not in the file at all.
        assert_eq!(sso_start_url(CONFIG, "keys"), None);
        assert_eq!(sso_start_url(CONFIG, "default"), None);
        assert_eq!(sso_start_url(CONFIG, "missing"), None);
    }

    #[test]
    fn a_cached_session_counts_only_while_it_has_comfortably_long_to_run() {
        let dir = tempdir();
        let url = "https://acme.awsapps.com/start";
        let now = chrono::Utc::now();
        let write = |name: &str, body: String| std::fs::write(dir.join(name), body).expect("write cache file");
        let entry = |expires: chrono::DateTime<chrono::Utc>| {
            format!(
                r#"{{"startUrl":"{url}","accessToken":"tok","expiresAt":"{}"}}"#,
                expires.to_rfc3339()
            )
        };

        // Nothing cached at all.
        assert!(!cached_session_is_live(&dir, url, now));

        // A registration blob, which has no session in it.
        write("registration.json", r#"{"clientId":"c","clientSecret":"s"}"#.to_string());
        assert!(!cached_session_is_live(&dir, url, now));

        // Expired, and expiring inside the margin: both need signing in again.
        write("old.json", entry(now - chrono::TimeDelta::hours(1)));
        assert!(!cached_session_is_live(&dir, url, now));
        write("nearly.json", entry(now + chrono::TimeDelta::seconds(30)));
        assert!(!cached_session_is_live(&dir, url, now));

        // Someone else's session does not count for this profile.
        write("other.json", entry(now + chrono::TimeDelta::hours(8)).replace(url, "https://other.awsapps.com/start"));
        assert!(!cached_session_is_live(&dir, url, now));

        write("good.json", entry(now + chrono::TimeDelta::hours(8)));
        assert!(cached_session_is_live(&dir, url, now));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_trailing_slash_does_not_make_it_a_different_start_url() {
        let expires = (chrono::Utc::now() + chrono::TimeDelta::hours(1)).to_rfc3339();
        let text = format!(r#"{{"startUrl":"https://acme.awsapps.com/start/","accessToken":"t","expiresAt":"{expires}"}}"#);
        assert!(session_expiry(&text, "https://acme.awsapps.com/start").is_some());
    }

    #[test]
    fn both_spellings_of_an_expiry_are_understood() {
        assert!(parse_expiry("2026-09-18T07:21:33Z").is_some());
        assert!(parse_expiry("2026-09-18T07:21:33UTC").is_some());
        assert!(parse_expiry("whenever").is_none());
    }

    #[test]
    fn only_credentials_failures_are_worth_signing_in_for() {
        let sso = anyhow!(
            "generate AWS RDS auth token: the SSO session associated with this profile has expired or is \
             otherwise invalid"
        );
        assert!(looks_like_credentials_problem(&sso));
        assert!(looks_like_credentials_problem(&anyhow!("no providers in chain provided credentials")));
        assert!(looks_like_credentials_problem(&anyhow!("ExpiredToken: the security token included in the request is expired")));
        // A genuine misconfiguration should be reported, not papered over with
        // a browser window.
        assert!(!looks_like_credentials_problem(&anyhow!("build AWS RDS auth token config: hostname is required")));
        assert!(!looks_like_credentials_problem(&anyhow!("dns error: failed to lookup address information")));
    }

    #[test]
    fn the_login_command_names_the_profile() {
        assert_eq!(sso_login_args("ops"), vec!["sso", "login", "--profile", "ops"]);
        assert_eq!(sso_login_args("default"), vec!["sso", "login", "--profile", "default"]);
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("multidb-sso-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
