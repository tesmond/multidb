use std::{
    collections::HashMap,
    future::Future,
    sync::{Mutex, OnceLock},
};

tokio::task_local! {
    static IPC_REQUEST_ID: String;
}

static TEST_CONNECTION_STAGES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn stages() -> &'static Mutex<HashMap<String, String>> {
    TEST_CONNECTION_STAGES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn with_request_context<F, T>(request_id: String, future: F) -> T
where
    F: Future<Output = T>,
{
    IPC_REQUEST_ID.scope(request_id, future).await
}

pub fn set_test_connection_stage(stage: &str) {
    if let Ok(request_id) = IPC_REQUEST_ID.try_with(|id| id.clone()) {
        if let Ok(mut map) = stages().lock() {
            map.insert(request_id, stage.to_string());
        }
    }
}

pub fn get_test_connection_stage(request_id: &str) -> Option<String> {
    stages().lock().ok()?.get(request_id).cloned()
}

pub fn clear_test_connection_stage(request_id: &str) {
    if let Ok(mut map) = stages().lock() {
        map.remove(request_id);
    }
}

pub fn current_test_connection_stage() -> Option<String> {
    let request_id = IPC_REQUEST_ID.try_with(|id| id.clone()).ok()?;
    get_test_connection_stage(&request_id)
}

pub fn recommended_checks_for_stage(stage: &str) -> &'static str {
    match stage {
        "prepare_runtime_connection_config" | "validate_connection_config" => {
            "Verify host, port, username, awsRegion, and authMode=awsIam; ensure Kubernetes port-forward is disabled for IAM."
        }
        "aws_sdk_load_config" => {
            "Run `aws sts get-caller-identity --profile <profile> --region <region>`; if using SSO run `aws sso login --profile <profile>`; try `AWS_EC2_METADATA_DISABLED=true` to avoid metadata-provider stalls."
        }
        "aws_generate_iam_token" => {
            "Confirm the DB username is IAM-enabled and matches exactly; verify AWS credentials permit token generation and region/endpoint are correct."
        }
        "mysql_build_connect_options" => {
            "If TLS CA bundle path is set, verify the file exists and is readable; otherwise leave CA path empty and retry with required TLS mode."
        }
        "mysql_connect" => {
            "Check network reachability to host:port (`nc -vz <host> <port>`), security groups/NACLs, VPN routing, and that the RDS endpoint is correct."
        }
        "mysql_validate" | "validate_connection_access" => {
            "Connection established but probe query stalled; verify server health/load, user permissions for `SELECT 1`, and proxy/firewall behavior."
        }
        "postgres_connect" | "sqlite_connect" => {
            "Validate driver-specific connectivity and credentials; confirm endpoint/path is reachable from the desktop process context."
        }
        _ => {
            "Capture backend logs around the test request and run `aws sts get-caller-identity` + `nc -vz <host> <port>` from the same shell/user as the app."
        }
    }
}
