//! AWS IAM authentication for RDS MySQL connections.
//!
//! An RDS IAM auth token is signed with the caller's AWS credentials and is
//! only good for fifteen minutes, so one is generated for every connect and
//! reconnect. Signing needs live credentials, and on the SSO profiles most
//! people use those come from an SSO session that outlasts them: the role
//! credentials and the SSO access token behind them lapse within the hour,
//! while the sign-in itself lasts for hours or days and lets the SDK mint new
//! ones without anybody being asked for anything.
//!
//! Before signing, check the SSO cache's expiry. An expired or nearly expired
//! SSO token triggers `aws sso login`, even if cached role credentials could
//! still sign an RDS token. Missing expiry metadata falls back to trying the
//! SDK and signing in if it reports a credentials failure.

use crate::ipc_diagnostics;
use crate::models::ConnectionConfig;
use anyhow::{anyhow, Context, Result};
use aws_config::{BehaviorVersion, Region};
use aws_sdk_rds::auth_token::{AuthTokenGenerator, Config as AuthTokenConfig};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

const SDK_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// `aws sso login` opens a browser and waits for the person to approve there,
/// so this is a human timeout rather than a network one.
const SSO_LOGIN_TIMEOUT: Duration = Duration::from_secs(180);
const SSO_REFRESH_BUFFER: Duration = Duration::from_secs(5 * 60);
/// Maximum lifetime of an RDS auth token.
pub const TOKEN_LIFETIME_SECS: u64 = 900;

/// Read the signing timestamp and lifetime from an RDS token.
pub(crate) fn token_expires_at(token: &str) -> Result<SystemTime> {
    // Never include the URL in errors: it contains the bearer credential.
    let url = url::Url::parse(&format!("https://{token}"))
        .map_err(|_| anyhow!("invalid AWS IAM token URL"))?;
    let params: HashMap<_, _> = url.query_pairs().collect();
    let signed_at = params
        .get("X-Amz-Date")
        .ok_or_else(|| anyhow!("AWS IAM token is missing its signing timestamp"))?;
    let signed_at = chrono::NaiveDateTime::parse_from_str(signed_at, "%Y%m%dT%H%M%SZ")
        .map_err(|_| anyhow!("AWS IAM token has an invalid signing timestamp"))?
        .and_utc();
    let lifetime: u64 = params
        .get("X-Amz-Expires")
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0 && *seconds <= TOKEN_LIFETIME_SECS)
        .ok_or_else(|| anyhow!("AWS IAM token has a missing or invalid lifetime"))?;
    let expires_at = signed_at
        .checked_add_signed(chrono::TimeDelta::seconds(lifetime as i64))
        .ok_or_else(|| anyhow!("AWS IAM token expiry is out of range"))?;
    Ok(expires_at.into())
}

/// Generate an RDS IAM token, renewing SSO first when its cached expiry is due.
pub async fn auth_token(cfg: &ConnectionConfig) -> Result<String> {
    let profile = AwsProfile::resolve(cfg);
    let attempt_started = Instant::now();
    auth_token_with(
        &profile,
        profile.cached_sso_expiry(),
        SystemTime::now(),
        || generate_token(cfg, &profile),
        || ensure_signed_in(&profile, attempt_started),
    )
    .await
}

async fn auth_token_with<G, GF, L, LF>(
    profile: &AwsProfile,
    cached_expiry: Option<SystemTime>,
    now: SystemTime,
    mut generate: G,
    login: L,
) -> Result<String>
where
    G: FnMut() -> GF,
    GF: Future<Output = Result<String>>,
    L: FnOnce() -> LF,
    LF: Future<Output = Result<()>>,
{
    if profile.is_sso() && sso_expiry_due(cached_expiry, now) {
        login()
            .await
            .with_context(|| format!("renew AWS SSO for profile {}", profile.name))?;
        // Load fresh SDK config after login; do not reuse old credential providers.
        return generate().await;
    }

    // Unknown cache metadata must not prevent environment/role credentials
    // from working. Retain the credentials-error fallback in that case.
    let first = match generate().await {
        Ok(token) => return Ok(token),
        Err(err) => err,
    };

    // Signing in cannot help a profile that has nothing to sign in to, or an
    // error that is not about credentials: report those as they are.
    if !profile.is_sso() || !looks_like_credentials_problem(&first) {
        return Err(first);
    }

    // The SDK could not get credentials and said so in terms of the session:
    // it has expired (or been revoked, or never started). One sign-in and one
    // retry.
    login()
        .await
        .with_context(|| format!("AWS credentials for profile {} could not be obtained ({first}), and signing in to AWS SSO failed", profile.name))?;
    generate().await
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

/// One `aws sso login` per session at a time. Opening several connections at
/// once — which is exactly what happens when the app restores a session — would
/// otherwise spawn a browser window each.
fn login_lock(profile: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().expect("aws login locks");
    Arc::clone(locks.entry(profile.to_string()).or_default())
}

/// When each session last finished an `aws sso login` from here.
fn last_logins() -> &'static Mutex<HashMap<String, Instant>> {
    static LOGINS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LOGINS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_login(profile: &str) -> Option<Instant> {
    last_logins().lock().ok()?.get(profile).copied()
}

fn record_login(profile: &str) {
    if let Ok(mut logins) = last_logins().lock() {
        logins.insert(profile.to_string(), Instant::now());
    }
}

/// Sign the profile in to SSO. `attempt_started` is when the caller began the
/// attempt that found the session wanting: if somebody else's sign-in finished
/// after that, the session is fresh and there is nothing to do, which is what
/// whoever gets the lock second finds when several connections fail together.
async fn ensure_signed_in(profile: &AwsProfile, attempt_started: Instant) -> Result<()> {
    ensure_signed_in_with(profile, attempt_started, || run_sso_login(profile)).await
}

async fn ensure_signed_in_with<L, LF>(
    profile: &AwsProfile,
    attempt_started: Instant,
    login: L,
) -> Result<()>
where
    L: FnOnce() -> LF,
    LF: Future<Output = Result<()>>,
{
    let key = profile
        .sso_session
        .as_deref()
        .or(profile.sso_start_url.as_deref())
        .ok_or_else(|| anyhow!("AWS profile {} is not configured for SSO", profile.name))?;
    let lock = login_lock(key);
    let _held = lock.lock().await;
    if last_login(key).is_some_and(|finished| finished >= attempt_started) {
        return Ok(());
    }
    login().await?;
    record_login(key);
    Ok(())
}

async fn run_sso_login(profile: &AwsProfile) -> Result<()> {
    let Some(start_url) = profile.sso_start_url.as_deref() else {
        return Err(anyhow!(
            "AWS profile {} is not configured for SSO, so its credentials cannot be renewed automatically. \
             Set up the profile in ~/.aws/config, or refresh its credentials yourself.",
            profile.name
        ));
    };
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
            // Abandoning the wait (a cancelled test, a timeout) must not leave
            // a login waiting on a browser tab nobody is looking at.
            .kill_on_drop(true)
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

// ─── Reading the profile ────────────────────────────────────────────────────

/// What this needs to know about the AWS profile a connection uses: its name,
/// and the SSO start URL when it signs in through SSO. The start URL's absence
/// is what says "this profile cannot be renewed by signing in".
#[derive(Debug, Clone, PartialEq)]
pub struct AwsProfile {
    pub name: String,
    pub sso_start_url: Option<String>,
    sso_session: Option<String>,
}

impl AwsProfile {
    fn resolve(cfg: &ConnectionConfig) -> Self {
        let name = profile_name(
            cfg.aws_profile.trim(),
            std::env::var("AWS_PROFILE").ok().as_deref(),
        );
        let config = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Self::from_config(&config, &name)
    }

    fn from_config(config: &str, name: &str) -> Self {
        let section = if name == "default" {
            "default".to_string()
        } else {
            format!("profile {name}")
        };
        let sso_session =
            ini_section(config, &section).and_then(|entries| entries.get("sso_session").cloned());
        Self {
            sso_start_url: sso_start_url(config, name),
            sso_session,
            name: name.to_string(),
        }
    }

    fn sso_cache_path(&self, home: &Path) -> Option<PathBuf> {
        // Match the AWS SDK/CLI exactly; scanning by startUrl can pick another
        // session with the same portal but a different expiry.
        let identifier = self
            .sso_session
            .as_deref()
            .or(self.sso_start_url.as_deref())?;
        let key = format!("{:x}", Sha1::digest(identifier.as_bytes()));
        Some(home.join(".aws/sso/cache").join(format!("{key}.json")))
    }

    fn cached_sso_expiry(&self) -> Option<SystemTime> {
        self.cached_sso_expiry_in(&dirs::home_dir()?)
    }

    fn cached_sso_expiry_in(&self, home: &Path) -> Option<SystemTime> {
        let content = std::fs::read(self.sso_cache_path(home)?).ok()?;
        // Never log cache contents or parse errors: this file contains tokens.
        let cache: serde_json::Value = serde_json::from_slice(&content).ok()?;
        let expiry = cache.get("expiresAt")?.as_str()?;
        let normalized = expiry.strip_suffix("UTC").map(|date| format!("{date}Z"));
        chrono::DateTime::parse_from_rfc3339(normalized.as_deref().unwrap_or(expiry))
            .ok()
            .map(SystemTime::from)
    }

    fn is_sso(&self) -> bool {
        self.sso_start_url.is_some()
    }
}

fn sso_expiry_due(expiry: Option<SystemTime>, now: SystemTime) -> bool {
    expiry.is_some_and(|expiry| {
        expiry
            .duration_since(now)
            .map_or(true, |remaining| remaining <= SSO_REFRESH_BUFFER)
    })
}

pub(crate) fn sso_login_due(cfg: &ConnectionConfig) -> bool {
    if !cfg.uses_aws_iam_auth() {
        return false;
    }
    let profile = AwsProfile::resolve(cfg);
    profile.is_sso() && sso_expiry_due(profile.cached_sso_expiry(), SystemTime::now())
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

    #[tokio::test]
    async fn expired_sso_logs_in_even_when_cached_role_credentials_can_still_sign() {
        let profile = AwsProfile {
            name: "overnight".into(),
            sso_start_url: Some("https://example.awsapps.com/start".into()),
            sso_session: None,
        };
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for expiry in [
            now - Duration::from_secs(86_400),
            now,
            now + SSO_REFRESH_BUFFER,
        ] {
            let logged_in = std::sync::atomic::AtomicBool::new(false);
            let result = auth_token_with(
                &profile,
                Some(expiry),
                now,
                || async {
                    assert!(
                        logged_in.load(std::sync::atomic::Ordering::SeqCst),
                        "expired SSO must be renewed before signing with cached role credentials"
                    );
                    Ok("new IAM token".into())
                },
                || async {
                    logged_in.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .unwrap();
            assert_eq!(result, "new IAM token");
        }
    }

    #[test]
    fn sso_cache_reads_the_exact_session_or_legacy_file_and_only_access_token_expiry() {
        let home = std::env::temp_dir().join(format!("multidb-sso-{}", uuid::Uuid::new_v4()));
        let cache = home.join(".aws/sso/cache");
        std::fs::create_dir_all(&cache).unwrap();
        let modern = AwsProfile::from_config(CONFIG, "modern");
        let legacy = AwsProfile::from_config(CONFIG, "legacy-sso");
        let modern_file = cache.join("293abb6b76d7791c0732cc517d38c4b5c734b87f.json");
        let legacy_file = cache.join("40169cea58ca6cbf23fb27b2542923fb5f524af0.json");
        assert_eq!(modern.sso_cache_path(&home).unwrap(), modern_file);
        assert_eq!(legacy.sso_cache_path(&home).unwrap(), legacy_file);
        assert!(modern.cached_sso_expiry_in(&home).is_none());
        std::fs::write(&modern_file, r#"{"expiresAt":"2026-10-15T08:00:00Z","registrationExpiresAt":"2030-01-01T00:00:00Z"}"#).unwrap();
        std::fs::write(&legacy_file, r#"{"expiresAt":"2026-10-15T10:00:00UTC"}"#).unwrap();
        let expected =
            SystemTime::from(chrono::DateTime::parse_from_rfc3339("2026-10-15T08:00:00Z").unwrap());
        assert_eq!(modern.cached_sso_expiry_in(&home), Some(expected));
        assert_eq!(
            legacy.cached_sso_expiry_in(&home),
            Some(expected + Duration::from_secs(7200))
        );
        // Re-read changes made by a CLI login while the app stays open.
        std::fs::write(&modern_file, r#"{"expiresAt":"2026-10-15T12:00:00+02:00"}"#).unwrap();
        assert_eq!(
            modern.cached_sso_expiry_in(&home),
            Some(expected + Duration::from_secs(7200))
        );
        for invalid in [
            "invalid json",
            r#"{"registrationExpiresAt":"2030-01-01T00:00:00Z"}"#,
            r#"{"expiresAt":"invalid"}"#,
        ] {
            std::fs::write(&modern_file, invalid).unwrap();
            assert!(modern.cached_sso_expiry_in(&home).is_none());
        }
        assert!(AwsProfile::from_config(CONFIG, "keys")
            .sso_cache_path(&home)
            .is_none());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn healthy_or_unknown_sso_expiry_keeps_working_credentials_without_login() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let profile = AwsProfile::from_config(CONFIG, "modern");
        for expiry in [
            None,
            Some(now + SSO_REFRESH_BUFFER + Duration::from_secs(1)),
        ] {
            let token = auth_token_with(
                &profile,
                expiry,
                now,
                || async { Ok("IAM token".into()) },
                || async { panic!("healthy credentials must not open a browser") },
            )
            .await
            .unwrap();
            assert_eq!(token, "IAM token");
        }
    }

    #[tokio::test]
    async fn sso_credentials_failure_still_logs_in_and_retries_with_fresh_credentials() {
        let profile = AwsProfile::from_config(CONFIG, "modern");
        let attempts = std::cell::Cell::new(0);
        let logged_in = std::cell::Cell::new(false);
        let token = auth_token_with(
            &profile,
            None,
            SystemTime::now(),
            || async {
                attempts.set(attempts.get() + 1);
                if !logged_in.get() {
                    Err(anyhow!("SSO session expired"))
                } else {
                    Ok("fresh IAM token".into())
                }
            },
            || async {
                logged_in.set(true);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(token, "fresh IAM token");
        assert_eq!(attempts.get(), 2);
    }

    #[tokio::test]
    async fn failed_sso_login_propagates_and_is_retryable() {
        let mut profile = AwsProfile::from_config(CONFIG, "modern");
        let session = format!("test-session-{}", uuid::Uuid::new_v4());
        profile.sso_session = Some(session.clone());
        let started = Instant::now();
        let error =
            ensure_signed_in_with(&profile, started, || async { Err(anyhow!("login failed")) })
                .await
                .unwrap_err();
        assert_eq!(error.to_string(), "login failed");
        assert!(last_login(&session).is_none());
        ensure_signed_in_with(&profile, started, || async { Ok(()) })
            .await
            .unwrap();
        assert!(last_login(&session).is_some());
    }

    #[tokio::test]
    async fn failed_proactive_sso_login_stops_signing_and_reports_the_failure() {
        let profile = AwsProfile::from_config(CONFIG, "modern");
        let now = SystemTime::now();
        let error = auth_token_with(
            &profile,
            Some(now),
            now,
            || async { panic!("must not sign using the expired session after login fails") },
            || async { Err(anyhow!("browser approval timed out")) },
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("browser approval timed out"));
    }

    #[tokio::test]
    async fn non_sso_profiles_and_non_credentials_errors_do_not_launch_login() {
        let now = SystemTime::now();
        let profile = AwsProfile::from_config(CONFIG, "keys");
        let token = auth_token_with(
            &profile,
            Some(now),
            now,
            || async { Ok("static-credentials IAM token".into()) },
            || async { panic!("static keys must not trigger SSO login") },
        )
        .await
        .unwrap();
        assert_eq!(token, "static-credentials IAM token");
        let profile = AwsProfile::from_config(CONFIG, "modern");
        let error = auth_token_with(
            &profile,
            None,
            now,
            || async { Err(anyhow!("DNS lookup failed")) },
            || async { panic!("network errors must not trigger SSO login") },
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "DNS lookup failed");
    }

    #[tokio::test]
    async fn profiles_sharing_an_sso_session_only_launch_one_login() {
        let mut first = AwsProfile::from_config(CONFIG, "modern");
        first.sso_session = Some(format!("test-session-{}", uuid::Uuid::new_v4()));
        let mut second = first.clone();
        second.name = "another-role".into();
        let started = Instant::now();
        let calls = std::cell::Cell::new(0);
        let login = || async {
            calls.set(calls.get() + 1);
            tokio::task::yield_now().await;
            Ok(())
        };
        let (a, b) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                ensure_signed_in_with(&first, started, login),
                ensure_signed_in_with(&second, started, login)
            )
        })
        .await
        .unwrap();
        a.unwrap();
        b.unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn iam_token_expiry_uses_the_signed_timestamp_and_lifetime() {
        let token = "db.example:3306/?X-Amz-Date=20261014T120000Z&X-Amz-Expires=900";
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-14T12:15:00Z").unwrap();
        assert_eq!(token_expires_at(token).unwrap(), SystemTime::from(expected));
        let shorter = token.replace("900", "120");
        assert_eq!(
            token_expires_at(&shorter).unwrap(),
            SystemTime::from(expected) - Duration::from_secs(780)
        );
    }

    #[test]
    fn iam_token_expiry_rejects_missing_or_invalid_metadata() {
        for query in [
            "X-Amz-Expires=900",
            "X-Amz-Date=20261014T120000Z",
            "X-Amz-Date=invalid&X-Amz-Expires=900",
            "X-Amz-Date=20261014T120000Z&X-Amz-Expires=invalid",
            "X-Amz-Date=20261014T120000Z&X-Amz-Expires=18446744073709551615",
        ] {
            assert!(token_expires_at(&format!("db.example:3306/?{query}")).is_err());
        }
    }

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
    fn a_sign_in_that_finished_after_an_attempt_began_satisfies_it() {
        let profile = format!("test-profile-{}", uuid::Uuid::new_v4());
        assert!(last_login(&profile).is_none());
        let began = Instant::now();
        record_login(&profile);
        assert!(last_login(&profile).is_some_and(|finished| finished >= began));
    }

    #[test]
    fn the_login_command_names_the_profile() {
        assert_eq!(sso_login_args("ops"), vec!["sso", "login", "--profile", "ops"]);
        assert_eq!(sso_login_args("default"), vec!["sso", "login", "--profile", "default"]);
    }
}
