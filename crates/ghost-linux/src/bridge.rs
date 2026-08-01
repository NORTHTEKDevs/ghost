//! Async-to-sync bridge for AT-SPI.
//!
//! `atspi` is asynchronous (zbus), while `ghost-session` presents a synchronous
//! API and `ghost-mcp` is already running inside a Tokio runtime. Calling
//! `block_on` from a runtime worker thread panics or deadlocks, so instead we
//! own a **dedicated OS thread** running its own current-thread runtime, and
//! ship work to it as boxed closures.
//!
//! This also solves a second, less obvious problem. An application services its
//! AT-SPI D-Bus methods on its own GUI main loop. If a process synchronously
//! waits on an AT-SPI request addressed to itself, it deadlocks. Keeping every
//! accessibility call on one isolated thread that never blocks the caller's
//! runtime removes that class of hang.
//!
//! Every job is bounded twice: by `tokio::time::timeout` inside the worker (so a
//! wedged application cannot block the queue forever) and by `recv_timeout` on
//! the caller side (so a wedged *worker* cannot block the MCP server).

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::time::Duration;

use atspi::AccessibilityConnection;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::error::{CoreError, Result};

/// Default per-call deadline. Deliberately short: a hung accessibility call
/// should surface as a retryable error, not a stalled agent.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5_000);

/// What a job receives: the live accessibility connection.
pub struct Ctx {
    pub conn: AccessibilityConnection,
}

impl Ctx {
    /// The underlying zbus connection, used to build interface proxies at a
    /// given (bus name, object path).
    pub fn zbus(&self) -> &zbus::Connection {
        self.conn.connection()
    }
}

type Job = Box<dyn for<'a> FnOnce(&'a Ctx) -> Pin<Box<dyn Future<Output = ()> + 'a>> + Send>;

/// Handle to the dedicated AT-SPI thread.
pub struct A11yBridge {
    tx: UnboundedSender<Job>,
}

impl A11yBridge {
    /// Start the worker thread and connect to the accessibility bus.
    ///
    /// Fails fast with a diagnosable error if the a11y bus is unreachable --
    /// which on a stock Fedora box almost always means accessibility is off.
    /// `ghost doctor` turns that into an actionable message.
    pub fn new() -> Result<Self> {
        let (tx, mut rx) = unbounded_channel::<Job>();
        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(1);

        std::thread::Builder::new()
            .name("ghost-a11y".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(CoreError::platform(format!(
                            "could not start accessibility runtime: {e}"
                        ))));
                        return;
                    }
                };

                rt.block_on(async move {
                    let conn = match AccessibilityConnection::new().await {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = ready_tx.send(Err(CoreError::platform(format!(
                                "cannot reach the AT-SPI accessibility bus ({e}). \
                                 Enable it with: gsettings set org.gnome.desktop.interface \
                                 toolkit-accessibility true  -- and ensure at-spi2-core is installed \
                                 and a desktop session is running."
                            ))));
                            return;
                        }
                    };

                    let ctx = Ctx { conn };
                    let _ = ready_tx.send(Ok(()));

                    while let Some(job) = rx.recv().await {
                        job(&ctx).await;
                    }
                });
            })
            .map_err(|e| CoreError::platform(format!("could not spawn a11y thread: {e}")))?;

        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| CoreError::JobTimeout)??;

        Ok(Self { tx })
    }

    /// Run an async closure on the accessibility thread and block for its result.
    ///
    /// Use the [`a11y!`] macro at call sites; it hides the pinning boilerplate.
    pub fn run<F, R>(&self, timeout: Duration, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a Ctx) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let (rtx, rrx) = sync_channel::<Result<R>>(1);

        let job: Job = Box::new(move |ctx| {
            Box::pin(async move {
                // The job's own Result is flattened here, so callers get a
                // single `Result<R>` rather than `Result<Result<R>>`.
                let outcome = match tokio::time::timeout(timeout, f(ctx)).await {
                    Ok(v) => v,
                    Err(_) => Err(CoreError::JobTimeout),
                };
                // Receiver may have already given up; that is not an error.
                let _ = rtx.send(outcome);
            })
        });

        self.tx
            .send(job)
            .map_err(|_| CoreError::WorkerPanic("a11y thread has stopped".into()))?;

        // Caller deadline is deliberately longer than the worker's, so the
        // worker's own timeout produces the (more specific) error first.
        match rrx.recv_timeout(timeout + Duration::from_millis(500)) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(CoreError::JobTimeout),
            Err(RecvTimeoutError::Disconnected) => {
                Err(CoreError::WorkerPanic("a11y thread dropped the job".into()))
            }
        }
    }

    /// [`A11yBridge::run`] with [`DEFAULT_TIMEOUT`].
    pub fn run_default<F, R>(&self, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a Ctx) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.run(DEFAULT_TIMEOUT, f)
    }
}

/// Ergonomic wrapper for [`A11yBridge::run`].
///
/// ```ignore
/// let names = a11y!(bridge, |ctx| {
///     let root = ctx.conn.root_accessible_on_registry().await?;
///     Ok(root.get_children().await?.len())
/// })?;
/// ```
#[macro_export]
macro_rules! a11y {
    ($bridge:expr, |$ctx:ident| $body:block) => {
        $bridge.run_default(move |$ctx| Box::pin(async move $body))
    };
    ($bridge:expr, $timeout:expr, |$ctx:ident| $body:block) => {
        $bridge.run($timeout, move |$ctx| Box::pin(async move $body))
    };
}
