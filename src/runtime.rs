//! The shared tokio runtime for backend async work (downloads, LLM calls,
//! shortcut handlers). Replaces `tauri::async_runtime`.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("handy-async")
            .build()
            .expect("Failed to start the tokio runtime")
    })
}

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future)
}

pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(f)
}

/// Runs `future` on the tokio runtime and resolves on any executor, so GPUI
/// foreground tasks can await tokio-only work such as `reqwest` calls.
pub async fn run<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    spawn(future).await.expect("tokio task panicked")
}

/// Blocks the current thread on `future`. Tests only: app code awaits.
#[cfg(test)]
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}
