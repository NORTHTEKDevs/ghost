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
//!
//! Jobs run one at a time, so a job can wait in the queue behind a slow one.
//! A job the caller has given up on (its deadline passed, it got `JobTimeout`)
//! never starts: the caller and the worker settle which of them owns the job
//! with one compare-and-swap, so a click or a value change cannot land after
//! the agent was told it failed. A state-changing job (`run_action`) also
//! re-checks the emergency stop right before it runs: a stop raised while it
//! waited in the queue cancels it.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atspi::AccessibilityConnection;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::error::{CoreError, Result};

/// Default per-call deadline. Deliberately short: a hung accessibility call
/// should surface as a retryable error, not a stalled agent.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5_000);

/// How much longer the caller waits than the worker's own timeout, so the
/// worker's (more specific) error arrives first.
const CALLER_GRACE: Duration = Duration::from_millis(500);

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

/// The work half: borrows the live connection and drives one operation.
type RunFn<C> = Box<dyn for<'a> FnOnce(&'a C) -> Pin<Box<dyn Future<Output = ()> + 'a>> + Send>;
/// The failure half: reports why the job did not run.
type FailFn = Box<dyn FnOnce(CoreError) + Send>;
/// Opens the connection jobs run against (lazily, retried until it succeeds).
type ConnectFn<C> = Box<dyn FnMut() -> Pin<Box<dyn Future<Output = Result<C>>>> + Send>;

/// Who owns a queued job: still waiting, taken by the worker, or abandoned by
/// a caller that stopped waiting. Moves out of `PENDING` exactly once.
const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const ABANDONED: u8 = 2;

/// A unit of work for the accessibility thread.
///
/// Carries two closures because the connection is established lazily: `run` is
/// used once a connection exists, and `fail` reports why the job did not run.
/// Both own a clone of the same reply channel, so the caller always gets an
/// answer rather than a dropped sender and an opaque "disconnected".
struct Job<C> {
    run: RunFn<C>,
    fail: FailFn,
    /// The caller stops waiting at this instant; a job still queued then is
    /// dropped instead of run.
    deadline: Instant,
    /// The job changes state in an application (invoke, set a value, focus):
    /// the emergency stop cancels it even after it was queued.
    mutates: bool,
    state: Arc<AtomicU8>,
}

fn bus_error(e: impl std::fmt::Display) -> CoreError {
    CoreError::platform(format!(
        "cannot reach the AT-SPI accessibility bus ({e}). Enable it with: \
         gsettings set org.gnome.desktop.interface toolkit-accessibility true \
         -- and ensure at-spi2-core is installed and a desktop session is running. \
         Applications must be restarted after enabling it."
    ))
}

/// Handle to a dedicated job thread whose jobs borrow a `C`.
pub struct Bridge<C> {
    tx: UnboundedSender<Job<C>>,
}

/// Handle to the dedicated AT-SPI thread.
pub type A11yBridge = Bridge<Ctx>;

impl Bridge<Ctx> {
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
        Self::spawn(
            Box::new(|| {
                Box::pin(async {
                    AccessibilityConnection::new().await.map(|conn| Ctx { conn }).map_err(bus_error)
                })
            }),
            crate::input::hotkey::is_stopped,
        )
    }
}

impl<C: 'static> Bridge<C> {
    /// Start a job thread. `stopped` is read right before each state-changing
    /// job runs.
    fn spawn(mut connect: ConnectFn<C>, stopped: fn() -> bool) -> Result<Self> {
        let (tx, mut rx) = unbounded_channel::<Job<C>>();
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
                    let mut ctx: Option<C> = None;

                    while let Some(job) = rx.recv().await {
                        // The caller gave up (or is about to): never start it.
                        if Instant::now() >= job.deadline
                            || job
                                .state
                                .compare_exchange(PENDING, RUNNING, Ordering::AcqRel, Ordering::Acquire)
                                .is_err()
                        {
                            job.state.store(ABANDONED, Ordering::Release);
                            (job.fail)(CoreError::JobTimeout);
                            continue;
                        }
                        if job.mutates && stopped() {
                            (job.fail)(CoreError::Stopped);
                            continue;
                        }
                        if ctx.is_none() {
                            match connect().await {
                                Ok(c) => ctx = Some(c),
                                Err(e) => {
                                    (job.fail)(e);
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

    /// Run a read-only async closure on the job thread and block for its result.
    ///
    /// Use the [`a11y!`] macro at call sites; it hides the pinning boilerplate.
    pub fn run<F, R>(&self, timeout: Duration, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a C) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>> + Send + 'static,
        R: Send + 'static,
    {
        self.submit(timeout, false, f)
    }

    /// [`Bridge::run`] for a job that changes application state (invoke, set
    /// a value, edit, focus): it does not run if the emergency stop was raised
    /// while it waited in the queue.
    pub fn run_action<F, R>(&self, timeout: Duration, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a C) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>> + Send + 'static,
        R: Send + 'static,
    {
        self.submit(timeout, true, f)
    }

    fn submit<F, R>(&self, timeout: Duration, mutates: bool, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a C) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>> + Send + 'static,
        R: Send + 'static,
    {
        let (rtx, rrx) = sync_channel::<Result<R>>(1);
        // Both arms of the job answer on the same channel, so the caller gets a
        // real error whether the work ran or the bus was unreachable.
        let fail_tx = rtx.clone();
        let state = Arc::new(AtomicU8::new(PENDING));

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
            deadline: Instant::now() + timeout + CALLER_GRACE,
            mutates,
            state: state.clone(),
        };

        self.tx
            .send(job)
            .map_err(|_| CoreError::WorkerPanic("a11y thread has stopped".into()))?;

        match rrx.recv_timeout(timeout + CALLER_GRACE) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                // Still queued: abandon it, so it never runs after this caller
                // was told it failed.
                if state
                    .compare_exchange(PENDING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Err(CoreError::JobTimeout);
                }
                // The worker started it just now: its own timeout bounds it,
                // so wait for the real outcome instead of misreporting it.
                match rrx.recv_timeout(timeout + CALLER_GRACE) {
                    Ok(r) => r,
                    Err(_) => Err(CoreError::JobTimeout),
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(CoreError::WorkerPanic("a11y thread dropped the job".into()))
            }
        }
    }

    /// [`Bridge::run`] with [`DEFAULT_TIMEOUT`].
    pub fn run_default<F, R>(&self, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a C) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>> + Send + 'static,
        R: Send + 'static,
    {
        self.run(DEFAULT_TIMEOUT, f)
    }

    /// [`Bridge::run_action`] with [`DEFAULT_TIMEOUT`].
    pub fn run_action_default<F, R>(&self, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&'a C) -> Pin<Box<dyn Future<Output = Result<R>> + 'a>> + Send + 'static,
        R: Send + 'static,
    {
        self.run_action(DEFAULT_TIMEOUT, f)
    }
}

/// Ergonomic wrapper for [`Bridge::run`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn bridge(stopped: fn() -> bool) -> Arc<Bridge<()>> {
        Arc::new(Bridge::spawn(Box::new(|| Box::pin(async { Ok(()) })), stopped).expect("bridge"))
    }

    /// Occupy the queue with a job that sleeps `ms`, from another thread.
    fn slow_job(b: &Arc<Bridge<()>>, ms: u64) -> std::thread::JoinHandle<Result<()>> {
        let b = b.clone();
        let h = std::thread::spawn(move || {
            b.run(Duration::from_secs(5), move |_| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    Ok(())
                })
            })
        });
        std::thread::sleep(Duration::from_millis(100));
        h
    }

    /// Wait until every job queued so far has had its turn.
    fn drain(b: &Arc<Bridge<()>>) {
        b.run(Duration::from_secs(5), |_| Box::pin(async { Ok(()) })).expect("queue drains");
    }

    #[test]
    fn an_action_whose_caller_gave_up_never_runs() {
        static FLAG: AtomicBool = AtomicBool::new(false);
        let b = bridge(|| false);
        let slow = slow_job(&b, 1_200);
        // Caller deadline 50ms + grace 500ms: well before the slow job ends.
        let r = b.run_action(Duration::from_millis(50), |_| {
            Box::pin(async {
                FLAG.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        slow.join().unwrap().expect("slow job");
        drain(&b);
        assert!(matches!(r, Err(CoreError::JobTimeout)), "caller should time out: {r:?}");
        assert!(!FLAG.load(Ordering::SeqCst), "the action ran after its caller got JobTimeout");
    }

    #[test]
    fn a_stop_raised_while_an_action_waits_cancels_it() {
        static FLAG: AtomicBool = AtomicBool::new(false);
        // Local to this test's bridge: never the shared stop.
        static STOP: AtomicBool = AtomicBool::new(false);
        let b = bridge(|| STOP.load(Ordering::SeqCst));
        let slow = slow_job(&b, 400);
        let r = std::thread::scope(|s| {
            let h = s.spawn(|| {
                b.run_action(Duration::from_secs(3), |_| {
                    Box::pin(async {
                        FLAG.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                })
            });
            std::thread::sleep(Duration::from_millis(100));
            STOP.store(true, Ordering::SeqCst);
            h.join().unwrap()
        });
        slow.join().unwrap().expect("slow job");
        assert!(matches!(r, Err(CoreError::Stopped)), "expected Stopped: {r:?}");
        assert!(!FLAG.load(Ordering::SeqCst), "the action ran after the stop");
        // Reads still work while stopped.
        assert_eq!(b.run(Duration::from_secs(1), |_| Box::pin(async { Ok(7) })).unwrap(), 7);
    }
}
