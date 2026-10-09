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
//! So the sign-in is the last thing tried, not the first. A token is always
//! generated straight away, and the AWS SDK renews whatever it can from the
//! session on its own. Only when it cannot, because the session itself has
//! expired or was never there, is `aws sso login` run (it opens a browser), and
//! the token generated again. A connection only fails when that fails too.

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
use std::time::{Duration, Instant};

const SDK_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// `aws sso login` opens a browser and waits for the person to approve there,
/// so this is a human timeout rather than a network one.
const SSO_LOGIN_TIMEOUT: Duration = Duration::from_secs(180);
/// How long an RDS auth token is valid for. `AWS_IAM_REFRESH_AGE` in
/// `connections` reconnects before this runs out.
pub const TOKEN_LIFETIME_SECS: u64 = 900;

/// Generate an RDS IAM auth token for `cfg`, signing in to AWS SSO only if the
/// credentials cannot be had without.
pub async fn auth_token(cfg: &ConnectionConfig) -> Result<String> {
    let profile = AwsProfile::resolve(cfg);

    // Try for a token before anything else. The SDK finds credentials by itself
    // (environment, instance role, a cached role session) and renews an expired
    // SSO access token from the SSO session's refresh token, so a profile whose
    // sign-in is still good costs nothing here, however long ago its last token
    // or access token ran out.
    let attempt_started = Instant::now();
    let first = match generate_token(cfg, &profile).await {
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
    ensure_signed_in(&profile, attempt_started)
        .await
        .with_context(|| format!("AWS credentials for profile {} could not be obtained ({first}), and signing in to AWS SSO failed", profile.name))?;
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

/// When each profile last finished an `aws sso login` from here.
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
    let Some(start_url) = profile.sso_start_url.as_deref() else {
        return Err(anyhow!(
            "AWS profile {} is not configured for SSO, so its credentials cannot be renewed automatically. \
             Set up the profile in ~/.aws/config, or refresh its credentials yourself.",
            profile.name
        ));
    };
    let lock = login_lock(&profile.name);
    let _held = lock.lock().await;
    if last_login(&profile.name).is_some_and(|finished| finished >= attempt_started) {
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
            // Abandoning the wait (a cancelled test, a timeout) must not leave
            // a login waiting on a browser tab nobody is looking at.
            .kill_on_drop(true)
            .output(),
    )
    .await?
    .with_context(|| format!("run {} sso login", program.display()))?;

    if output.status.success() {
        record_login(&profile.name);
    } else {
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
