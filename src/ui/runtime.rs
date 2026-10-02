//! Bridge between GPUI's foreground executor and the tokio runtime that the
//! database backend (sqlx, kube port-forwards, AWS SDK) requires.

use crate::state::AppState;
use std::future::Future;
use std::sync::{Arc, OnceLock};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static STATE: OnceLock<Arc<AppState>> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("multidb-worker")
            .build()
            .expect("failed to start async runtime")
    })
}

pub fn state() -> Arc<AppState> {
    STATE.get_or_init(|| Arc::new(AppState::default())).clone()
}

/// Run `fut` on tokio; the returned future resolves on whichever executor
/// awaits it (normally a GPUI `cx.spawn`).
pub fn spawn<F, T>(fut: F) -> impl Future<Output = T> + Send + 'static
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = runtime().spawn(fut);
    async move {
        match handle.await {
            Ok(value) => value,
            Err(err) => std::panic::resume_unwind(err.into_panic()),
        }
    }
}

/// Run blocking work (native file dialogs) on tokio's blocking pool.
pub fn spawn_blocking<F, T>(f: F) -> impl Future<Output = T> + Send + 'static
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let handle = runtime().spawn_blocking(f);
    async move {
        match handle.await {
            Ok(value) => value,
            Err(err) => std::panic::resume_unwind(err.into_panic()),
        }
    }
}
