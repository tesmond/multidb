use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Default)]
pub struct StartupProfiler {
    inner: Option<Arc<StartupProfilerInner>>,
}

struct StartupProfilerInner {
    started: Instant,
    output_path: PathBuf,
    events: Mutex<Vec<StartupEvent>>,
}

#[derive(Serialize)]
struct StartupEvent {
    name: String,
    ms: f64,
    detail: Value,
}

impl StartupProfiler {
    pub fn from_env() -> Self {
        if std::env::var("MULTIDB_STARTUP_PROFILE").ok().as_deref() != Some("1") {
            return Self::default();
        }

        let output_path = std::env::var("MULTIDB_STARTUP_PROFILE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("multidb-startup-profile.json"));

        Self {
            inner: Some(Arc::new(StartupProfilerInner {
                started: Instant::now(),
                output_path,
                events: Mutex::new(Vec::new()),
            })),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn mark(&self, name: impl Into<String>) {
        self.mark_with(name, Value::Null);
    }

    pub fn mark_with(&self, name: impl Into<String>, detail: Value) {
        let Some(inner) = &self.inner else {
            return;
        };

        let event = StartupEvent {
            name: name.into(),
            ms: inner.started.elapsed().as_secs_f64() * 1000.0,
            detail,
        };

        if let Ok(mut events) = inner.events.lock() {
            events.push(event);
        }
    }

    pub fn mark_asset(&self, path: &str, source: &str, bytes: usize) {
        self.mark_with(
            "asset_served",
            json!({
                "path": path,
                "source": source,
                "bytes": bytes,
            }),
        );
    }

    pub fn should_exit_after_profile(&self) -> bool {
        self.enabled()
            && std::env::var("MULTIDB_STARTUP_PROFILE_EXIT")
                .ok()
                .as_deref()
                == Some("1")
    }

    pub fn write_report(&self, frontend: Value) -> Result<PathBuf> {
        let Some(inner) = &self.inner else {
            return Ok(PathBuf::new());
        };

        self.mark("frontend_profile_received");
        let events = inner
            .events
            .lock()
            .map(|events| json!(&*events))
            .unwrap_or_else(|_| json!([]));

        let report = json!({
            "profile": "multidb-startup",
            "generatedAt": chrono_like_timestamp(),
            "nativeEvents": events,
            "frontend": frontend,
        });

        if let Some(parent) = inner.output_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create profile directory {}", parent.display()))?;
        }
        std::fs::write(&inner.output_path, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("write profile {}", inner.output_path.display()))?;

        Ok(inner.output_path.clone())
    }
}

fn chrono_like_timestamp() -> String {
    // Avoid adding a timestamp dependency just for optional profiling output.
    format!("{:?}", std::time::SystemTime::now())
}
