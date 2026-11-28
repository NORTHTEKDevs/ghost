use crate::error::CoreError;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::IUIAutomation;

type Job = Box<
    dyn FnOnce(&IUIAutomation) -> Result<serde_json::Value, CoreError> + Send,
>;
type JobEnvelope = (Job, oneshot::Sender<Result<serde_json::Value, CoreError>>, Arc<AtomicU8>);

/// A job's state, shared by the caller and the worker so exactly one orphan
/// is counted and uncounted per abandoned job, however the caller went away
/// (the pool's timeout, an outer timeout, a dropped future).
const JOB_PENDING: u8 = 0;
const JOB_DONE: u8 = 1;
const JOB_ABANDONED: u8 = 2;

/// Marks the job abandoned when the caller stops waiting before it finished,
/// and counts it as an orphan. The count goes up before the state flips, so the
/// worker (which uncounts only after it sees ABANDONED) can never take the
/// counter below zero.
struct AbandonGuard {
    state: Arc<AtomicU8>,
    orphaned: Arc<AtomicUsize>,
}

impl AbandonGuard {
    /// True when this call counted the job as an orphan.
    fn abandon(&self) -> bool {
        self.orphaned.fetch_add(1, Ordering::AcqRel);
        if self
            .state
            .compare_exchange(JOB_PENDING, JOB_ABANDONED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            true
        } else {
            self.orphaned.fetch_sub(1, Ordering::AcqRel);
            false
        }
    }
}

impl Drop for AbandonGuard {
    fn drop(&mut self) {
        // No-op when the job already finished (state DONE) or was abandoned.
        self.abandon();
    }
}

const PANIC_WINDOW: Duration = Duration::from_secs(60);
const PANIC_THRESHOLD: usize = 3;
const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(30);

pub struct StaPool {
    tx: Sender<JobEnvelope>,
    panic_log: Arc<Mutex<VecDeque<Instant>>>,
    circuit_open: Arc<AtomicBool>,
    job_timeout: Duration,
    /// MEDIUM-9: count orphaned jobs (timed out but still executing in worker).
    /// Exposed via `health()` for observability; non-zero means concurrency is reduced.
    orphaned_jobs: Arc<AtomicUsize>,
}

impl StaPool {
    pub fn new(workers: usize) -> Result<Self, CoreError> {
        Self::with_timeout(workers, DEFAULT_JOB_TIMEOUT)
    }

    pub fn with_timeout(workers: usize, job_timeout: Duration) -> Result<Self, CoreError> {
        let (tx, rx) = unbounded::<JobEnvelope>();
        let panic_log = Arc::new(Mutex::new(VecDeque::<Instant>::new()));
        let circuit_open = Arc::new(AtomicBool::new(false));
        let orphaned_jobs = Arc::new(AtomicUsize::new(0));
        for i in 0..workers {
            Self::spawn_worker(i, rx.clone(), panic_log.clone(), circuit_open.clone(), orphaned_jobs.clone())?;
        }
        Ok(Self {
            tx,
            panic_log,
            circuit_open,
            job_timeout,
            orphaned_jobs,
        })
    }

    /// Return the number of currently orphaned (timed-out) jobs still executing.
    /// Non-zero values indicate reduced pool concurrency.
    pub fn orphaned_job_count(&self) -> usize {
        self.orphaned_jobs.load(Ordering::Acquire)
    }

    fn spawn_worker(
        id: usize,
        rx: Receiver<JobEnvelope>,
        panic_log: Arc<Mutex<VecDeque<Instant>>>,
        circuit_open: Arc<AtomicBool>,
        orphaned_jobs: Arc<AtomicUsize>,
    ) -> Result<(), CoreError> {
        thread::Builder::new()
            .name(format!("ghost-sta-{id}"))
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
                }
                // Same deadline-carrying constructor as the session tree.
                let uia: IUIAutomation =
                    crate::uia::create_automation().expect("CUIAutomation8");
                while let Ok((job, reply, state)) = rx.recv() {
                    let uia_ref = &uia;
                    let result = catch_unwind(AssertUnwindSafe(|| job(uia_ref)));
                    let reply_value = match result {
                        Ok(r) => r,
                        Err(panic_payload) => {
                            let msg = extract_panic_msg(&panic_payload);
                            record_panic(&panic_log, &circuit_open);
                            tracing::warn!("ghost-sta-{id} caught panic: {msg}");
                            Err(CoreError::WorkerPanic(msg))
                        }
                    };
                    // MEDIUM-9: an orphan is uncounted only if its caller counted
                    // it (state ABANDONED), never on a failed send alone.
                    if state.swap(JOB_DONE, Ordering::AcqRel) == JOB_ABANDONED {
                        let remaining = orphaned_jobs.fetch_sub(1, Ordering::AcqRel).saturating_sub(1);
                        tracing::warn!(worker = id, orphaned_remaining = remaining, "ghost-sta: orphaned job completed (caller stopped waiting)");
                    }
                    let _ = reply.send(reply_value);
                }
                // Release the automation object BEFORE the apartment goes away.
                // `uia` would otherwise drop at the end of this closure, after
                // `CoUninitialize`, and a `Release` on a torn-down apartment is
                // undefined behaviour: it is the intermittent
                // STATUS_ACCESS_VIOLATION seen in `cargo test -p ghost-core --lib`
                // on windows-latest while the pool tests (which drop pools) were
                // in flight. Same ordering rule as `Desktop::with_uia`.
                drop(uia);
                unsafe {
                    CoUninitialize();
                }
            })
            .map_err(|e| CoreError::ComInit(format!("spawn STA worker: {e}")))?;
        Ok(())
    }

    pub async fn submit<F, T>(&self, f: F) -> Result<T, CoreError>
    where
        F: FnOnce(&IUIAutomation) -> Result<T, CoreError> + Send + 'static,
        T: serde::de::DeserializeOwned + serde::Serialize + Send + 'static,
    {
        if self.circuit_open.load(Ordering::Acquire) {
            let mut log = self.panic_log.lock().unwrap();
            log.retain(|t| t.elapsed() < PANIC_WINDOW);
            if log.len() < PANIC_THRESHOLD {
                self.circuit_open.store(false, Ordering::Release);
            } else {
                return Err(CoreError::CircuitOpen);
            }
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        let job: Job = Box::new(move |uia| {
            let v = f(uia)?;
            serde_json::to_value(v).map_err(|e| {
                CoreError::ComInit(format!("serialize pool result: {e}"))
            })
        });
        let state = Arc::new(AtomicU8::new(JOB_PENDING));
        self.tx
            .send((job, reply_tx, state.clone()))
            .map_err(|_| CoreError::ComInit("pool dead".into()))?;
        // Counts the orphan if this future is dropped before the reply (a
        // cancelled MCP call) as well as on the timeout below.
        let guard = AbandonGuard { state, orphaned: self.orphaned_jobs.clone() };

        match tokio::time::timeout(self.job_timeout, reply_rx).await {
            Ok(Ok(res)) => {
                let raw = res?;
                serde_json::from_value(raw).map_err(|e| {
                    CoreError::ComInit(format!("deserialize pool result: {e}"))
                })
            }
            Ok(Err(_)) => Err(CoreError::ComInit("worker cancel".into())),
            Err(_) => {
                // MEDIUM-9: track orphaned jobs; log at error level so alerting can fire.
                guard.abandon();
                let orphaned = self.orphaned_jobs.load(Ordering::Acquire);
                tracing::error!(
                    timeout_ms = self.job_timeout.as_millis(),
                    orphaned_jobs = orphaned,
                    "ghost-sta: job timed out — worker is executing orphaned job (effective concurrency reduced)"
                );
                Err(CoreError::JobTimeout)
            }
        }
    }
}

fn record_panic(log: &Mutex<VecDeque<Instant>>, flag: &AtomicBool) {
    let mut log = log.lock().unwrap();
    let now = Instant::now();
    log.push_back(now);
    while let Some(front) = log.front() {
        if now.duration_since(*front) > PANIC_WINDOW {
            log.pop_front();
        } else {
            break;
        }
    }
    if log.len() >= PANIC_THRESHOLD {
        flag.store(true, Ordering::Release);
    }
}

fn extract_panic_msg(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_runs_closure_on_worker() {
        let pool = StaPool::new(2).unwrap();
        let result = pool.submit(|_uia| Ok(42i32)).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn pool_recovers_from_worker_panic() {
        let pool = StaPool::new(1).unwrap();
        let err = pool.submit::<_, i32>(|_| panic!("boom")).await;
        assert!(matches!(err, Err(CoreError::WorkerPanic(_))));
        let ok = pool.submit(|_| Ok(7i32)).await.unwrap();
        assert_eq!(ok, 7);
    }

    #[tokio::test]
    async fn pool_enforces_per_job_timeout() {
        let pool = StaPool::with_timeout(1, Duration::from_millis(100)).unwrap();
        let err = pool
            .submit::<_, ()>(|_| {
                std::thread::sleep(Duration::from_millis(500));
                Ok(())
            })
            .await;
        assert!(matches!(err, Err(CoreError::JobTimeout)));
    }

    /// Waits up to 5 s for the orphan count to reach `want`.
    async fn orphans_reach(pool: &StaPool, want: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let n = pool.orphaned_job_count();
            if n == want || Instant::now() > deadline {
                return n;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// A caller that goes away without the pool's own timeout (an MCP cancel,
    /// an outer `tokio::time::timeout`, an aborted task) leaves an orphaned
    /// job too. It used to be decremented without ever being counted, which
    /// wrapped the counter to ~usize::MAX.
    #[tokio::test]
    async fn a_cancelled_caller_counts_its_orphan_and_uncounts_it_once() {
        let pool = StaPool::with_timeout(1, Duration::from_secs(30)).unwrap();
        let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
        let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
        let mut call = Box::pin(pool.submit::<_, ()>(move |_| {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            Ok(())
        }));
        // Drive the call until the job is running on the worker, then drop it.
        tokio::select! {
            _ = &mut call => panic!("job finished before it was released"),
            _ = async {
                while started_rx.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            } => {}
        }
        drop(call); // the boxed future itself, as an MCP cancel drops it
        assert_eq!(pool.orphaned_job_count(), 1, "a cancelled call's running job is an orphan");
        release_tx.send(()).unwrap();
        assert_eq!(orphans_reach(&pool, 0).await, 0, "the orphan is uncounted once it finishes");
    }

    #[tokio::test]
    async fn a_timed_out_job_is_counted_until_it_finishes() {
        let pool = StaPool::with_timeout(1, Duration::from_millis(20)).unwrap();
        let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
        let err = pool
            .submit::<_, ()>(move |_| {
                let _ = release_rx.recv();
                Ok(())
            })
            .await;
        assert!(matches!(err, Err(CoreError::JobTimeout)));
        assert_eq!(pool.orphaned_job_count(), 1);
        release_tx.send(()).unwrap();
        assert_eq!(orphans_reach(&pool, 0).await, 0);
        // A normal call afterwards leaves the count alone.
        assert_eq!(pool.submit(|_| Ok(3i32)).await.unwrap(), 3);
        assert_eq!(pool.orphaned_job_count(), 0);
    }

    #[tokio::test]
    async fn pool_circuit_breaker_trips_after_three_panics_in_60s() {
        let pool = StaPool::new(1).unwrap();
        for _ in 0..3 {
            let _ = pool.submit::<_, ()>(|_| panic!("b")).await;
        }
        let err = pool.submit(|_| Ok(1i32)).await;
        assert!(matches!(err, Err(CoreError::CircuitOpen)));
    }
}
