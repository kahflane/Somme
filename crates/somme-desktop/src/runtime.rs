use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Starts the Tokio runtime that SSH, SFTP and the database run on.
/// GPUI owns the main thread, so all network and disk work happens here.
pub fn init() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("somme-io")
        .build()?;
    let _ = RUNTIME.set(runtime);
    Ok(())
}

pub fn handle() -> Handle {
    RUNTIME.get().expect("runtime::init was not called").handle().clone()
}

/// Runs `future` on the IO runtime. The returned future can be awaited from
/// GPUI's executors, which cannot drive Tokio-based code themselves.
pub fn io<F, T>(future: F) -> impl Future<Output = T> + 'static
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let task = handle().spawn(future);
    async move {
        match task.await {
            Ok(value) => value,
            Err(err) => match err.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(err) => panic!("io task was cancelled: {err}"),
            },
        }
    }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    RUNTIME.get().expect("runtime::init was not called").block_on(future)
}
