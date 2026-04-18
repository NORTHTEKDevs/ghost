use crate::error::CoreError;
use crossbeam_channel::{unbounded, Sender};
use std::thread;
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

pub struct StaPool {
    tx: Sender<JobEnvelope>,
}

impl StaPool {
    pub fn new(workers: usize) -> Result<Self, CoreError> {
        let (tx, rx) = unbounded::<JobEnvelope>();
        for i in 0..workers {
            let rx = rx.clone();
            thread::Builder::new()
                .name(format!("ghost-sta-{i}"))
                .spawn(move || {
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
                    }
                    let uia: IUIAutomation = unsafe {
                        CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                    }
                    .expect("CUIAutomation8");
                    while let Ok((job, reply)) = rx.recv() {
                        let res = job(&uia);
                        let _ = reply.send(res);
                    }
                    unsafe {
                        CoUninitialize();
                    }
                })
                .map_err(|e| CoreError::ComInit(format!("spawn STA worker: {e}")))?;
        }
        Ok(Self { tx })
    }

    pub async fn submit<F, T>(&self, f: F) -> Result<T, CoreError>
    where
        F: FnOnce(&IUIAutomation) -> Result<T, CoreError> + Send + 'static,
        T: serde::de::DeserializeOwned + serde::Serialize + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let job: Job = Box::new(move |uia| {
            let v = f(uia)?;
            Ok(serde_json::to_value(v).map_err(|e| {
                CoreError::ComInit(format!("serialize pool result: {e}"))
            })?)
        });
        self.tx
            .send((job, reply_tx))
            .map_err(|_| CoreError::ComInit("pool dead".into()))?;
        let raw = reply_rx
            .await
            .map_err(|_| CoreError::ComInit("worker cancel".into()))??;
        serde_json::from_value(raw)
            .map_err(|e| CoreError::ComInit(format!("deserialize pool result: {e}")))
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
}
