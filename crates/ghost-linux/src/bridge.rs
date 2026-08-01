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

/// A unit of work for the accessibility thread.
///
/// Carries two closures because the connection is established lazily: `run` is
/// used once a connection exists, and `fail` reports why one could not be made.
/// Both own a clone of the same reply channel, so the caller always gets an
/// answer rather than a dropped sender and an opaque "disconnected".
/// The work half: borrows the live connection and drives one operation.
type RunFn = Box<dyn for<'a> FnOnce(&'a Ctx) -> Pin<Box<dyn Future<Output = ()> + 'a>> + Send>;
/// The failure half: reports why no connection could be made.
type FailFn = Box<dyn FnOnce(CoreError) + Send>;

struct Job {
    run: RunFn,
    fail: FailFn,
}

fn bus_error(e: impl std::fmt::Display) -> CoreError {
    CoreError::platform(format!(
        "cannot reach the AT-SPI accessibility bus ({e}). Enable it with: \
         gsettings set org.gnome.desktop.interface toolkit-accessibility true \
         -- and ensure at-spi2-core is installed and a desktop session is running. \
         Applications must be restarted after enabling it."
    ))
}

/// Handle to the dedicated AT-SPI thread.
pub struct A11yBridge {
    tx: UnboundedSender<Job>,
}

impl A11yBridge {
    /// Start the worker thread.
    ///
    /// The accessibility bus is connected **lazily, on first use**, and this
    /// call does not fail when the bus is unreachable. That matters: the MCP
    /// server must start and answer `tools/list` on a machine where
    /// accessibility happens to be off, so the user sees an actionable error
    /// from the first real call instead of a server that refuses to boot with
    /// no explanation. `ghost doctor` reports the same condition directly.
    ///
    /// A failed connection is not cached, so enabling accessibility takes
    /// effect on the next call without restarting Ghost.
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
                // The thread and its runtime are up; that is all `new` promises.
                let _ = ready_tx.send(Ok(()));

                rt.block_on(async move {
                    let mut ctx: Option<Ctx> = None;

                    while let Some(job) = rx.recv().await {
                        if ctx.is_none() {
                            match AccessibilityConnection::new().await {
                                Ok(conn) => ctx = Some(Ctx { conn }),
                                Err(e) => {
                                    (job.fail)(bus_error(e));
                                    continue;
                                }
                            }
                        }
                        // Safe: just populated above, and never cleared.
                        if let Some(c) = ctx.as_ref() {
                            (job.run)(c).await;
                        }
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
        // Both arms of the job answer on the same channel, so the caller gets a
        // real error whether the work ran or the bus was unreachable.
        let fail_tx = rtx.clone();

        let job = Job {
            run: Box::new(move |ctx| {
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
            }),
            fail: Box::new(move |e| {
                let _ = fail_tx.send(Err(e));
            }),
        };

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
