use crate::error::CoreError;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation};

type Job = Box<
    dyn FnOnce(&IUIAutomation) -> Result<serde_json::Value, CoreError> + Send,
>;
type JobEnvelope = (Job, oneshot::Sender<Result<serde_json::Value, CoreError>>);

const PANIC_WINDOW: Duration = Duration::from_secs(60);
const PANIC_THRESHOLD: usize = 3;
const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(30);

pub struct StaPool {
    tx: Sender<JobEnvelope>,
    panic_log: Arc<Mutex<VecDeque<Instant>>>,
    circuit_open: Arc<AtomicBool>,
    job_timeout: Duration,
}

impl StaPool {
    pub fn new(workers: usize) -> Result<Self, CoreError> {
        Self::with_timeout(workers, DEFAULT_JOB_TIMEOUT)
    }

    pub fn with_timeout(workers: usize, job_timeout: Duration) -> Result<Self, CoreError> {
        let (tx, rx) = unbounded::<JobEnvelope>();
        let panic_log = Arc::new(Mutex::new(VecDeque::<Instant>::new()));
        let circuit_open = Arc::new(AtomicBool::new(false));
        for i in 0..workers {
            Self::spawn_worker(i, rx.clone(), panic_log.clone(), circuit_open.clone())?;
        }
        Ok(Self {
            tx,
            panic_log,
            circuit_open,
            job_timeout,
        })
    }

    fn spawn_worker(
        id: usize,
        rx: Receiver<JobEnvelope>,
        panic_log: Arc<Mutex<VecDeque<Instant>>>,
        circuit_open: Arc<AtomicBool>,
    ) -> Result<(), CoreError> {
        thread::Builder::new()
            .name(format!("ghost-sta-{id}"))
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
                }
                let uia: IUIAutomation = unsafe {
                    CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                }
                .expect("CUIAutomation8");
                while let Ok((job, reply)) = rx.recv() {
                    let uia_ref = &uia;
                    let result = catch_unwind(AssertUnwindSafe(|| job(uia_ref)));
                    match result {
                        Ok(r) => {
                            let _ = reply.send(r);
                        }
                        Err(panic_payload) => {
                            let msg = extract_panic_msg(&panic_payload);
                            record_panic(&panic_log, &circuit_open);
                            tracing::warn!("ghost-sta-{id} caught panic: {msg}");
                            let _ = reply.send(Err(CoreError::WorkerPanic(msg)));
                        }
                    }
                }
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
        self.tx
            .send((job, reply_tx))
            .map_err(|_| CoreError::ComInit("pool dead".into()))?;

        match tokio::time::timeout(self.job_timeout, reply_rx).await {
            Ok(Ok(res)) => {
                let raw = res?;
                serde_json::from_value(raw).map_err(|e| {
                    CoreError::ComInit(format!("deserialize pool result: {e}"))
                })
            }
            Ok(Err(_)) => Err(CoreError::ComInit("worker cancel".into())),
            Err(_) => {
                tracing::warn!(
                    "orphaning STA job after {:?} timeout",
                    self.job_timeout
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
