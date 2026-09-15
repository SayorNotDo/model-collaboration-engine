//! JSON boundary keeps the Python API independent of provider SDK types.
use crate::{
    contracts::*,
    engine::Engine,
    events::Event,
    tools::{ToolExecutor, ToolRequest},
};
use async_trait::async_trait;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

fn py_error(error: EngineError) -> PyErr {
    PyRuntimeError::new_err(serde_json::to_string(&error).unwrap())
}

#[pyclass(name = "Engine")]
struct PythonEngine {
    inner: Arc<Engine>,
}

struct HostWork {
    request: ToolRequest,
    reply: oneshot::Sender<Result<ToolResult>>,
}

struct HostBridge {
    sender: mpsc::Sender<HostWork>,
}

#[async_trait]
impl ToolExecutor for HostBridge {
    async fn execute(&self, request: ToolRequest) -> Result<ToolResult> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .send(HostWork { request, reply })
            .await
            .map_err(|_| EngineError::new("tool", "host tool receiver closed"))?;
        receiver
            .await
            .map_err(|_| EngineError::new("tool", "host tool reply channel closed"))?
    }
}

/// Separate control and event channels keep tool replies independent of stream consumption.
#[pyclass(name = "Session")]
struct PythonSession {
    events: Arc<tokio::sync::Mutex<mpsc::Receiver<Event>>>,
    work: Arc<tokio::sync::Mutex<mpsc::Receiver<HostWork>>>,
    replies: Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<ToolResult>>>>>,
    outcome: watch::Receiver<Option<Result<TaskResult>>>,
    cancel: CancellationToken,
}

impl Drop for PythonSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[pymethods]
impl PythonSession {
    fn cancel(&self) {
        self.cancel.cancel();
    }

    fn next_event<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let events = self.events.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Ok(events
                .lock()
                .await
                .recv()
                .await
                .map(|event| serde_json::to_string(&event).unwrap()))
        })
    }

    fn next_tool<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let work = self.work.clone();
        let replies = self.replies.clone();
        let cancel = self.cancel.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut receiver = work.lock().await;
            loop {
                let work = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => None,
                    work = receiver.recv() => work,
                };
                let Some(work) = work else {
                    replies.lock().unwrap().clear();
                    return Ok(None);
                };
                if work.reply.is_closed() || work.request.deadline_ms <= now_ms() {
                    continue;
                }
                let request = serde_json::to_string(&work.request).unwrap();
                replies
                    .lock()
                    .unwrap()
                    .retain(|_, reply| !reply.is_closed());
                replies
                    .lock()
                    .unwrap()
                    .insert(work.request.execution_id, work.reply);
                return Ok(Some(request));
            }
        })
    }

    fn reply(&self, execution_id: String, result_json: String) -> PyResult<bool> {
        let result: ToolResult = serde_json::from_str(&result_json)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(self
            .replies
            .lock()
            .unwrap()
            .remove(&execution_id)
            .is_some_and(|reply| reply.send(Ok(result)).is_ok()))
    }

    fn fail_tool(&self, execution_id: String, message: String) -> bool {
        self.replies
            .lock()
            .unwrap()
            .remove(&execution_id)
            .is_some_and(|reply| reply.send(Err(EngineError::new("tool", message))).is_ok())
    }

    fn result<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut outcome = self.outcome.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            loop {
                let current = outcome.borrow().clone();
                if let Some(result) = current {
                    return result
                        .map(|r| serde_json::to_string(&r).unwrap())
                        .map_err(py_error);
                }
                outcome.changed().await.map_err(|_| {
                    PyRuntimeError::new_err("engine worker exited without a result")
                })?;
            }
        })
    }
}

#[pyclass(name = "Cancellation")]
struct PythonCancellation {
    token: CancellationToken,
}

#[pymethods]
impl PythonCancellation {
    #[new]
    fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }
    fn cancel(&self) {
        self.token.cancel();
    }
}

impl PythonEngine {
    fn start_session(&self, input: SubmissionSpec, with_tools: bool) -> PyResult<PythonSession> {
        let engine = self.inner.clone();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (sender, events) = engine.event_channel();
        let (host_sender, work) = mpsc::channel(1);
        let tools: Option<Arc<dyn ToolExecutor>> = if with_tools {
            Some(Arc::new(HostBridge {
                sender: host_sender,
            }))
        } else {
            drop(host_sender);
            None
        };
        let (finished, outcome) = watch::channel(None);
        pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
            let result = engine
                .run_with_host(input, worker_cancel, tools, Some(sender))
                .await;
            let _ = finished.send(Some(result));
        });
        Ok(PythonSession {
            events: Arc::new(tokio::sync::Mutex::new(events)),
            work: Arc::new(tokio::sync::Mutex::new(work)),
            replies: Arc::new(Mutex::new(BTreeMap::new())),
            outcome,
            cancel,
        })
    }
}

#[pymethods]
impl PythonEngine {
    fn start(&self, task_json: String, with_tools: bool) -> PyResult<PythonSession> {
        let task =
            serde_json::from_str(&task_json).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        self.start_session(task, with_tools)
    }

    #[staticmethod]
    fn open(py: Python<'_>, config_json: String) -> PyResult<Bound<'_, PyAny>> {
        let config: Config = serde_json::from_str(&config_json)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Ok(Self {
                inner: Arc::new(Engine::open(config).await.map_err(py_error)?),
            })
        })
    }

    fn run<'py>(
        &self,
        py: Python<'py>,
        task_json: String,
        cancellation: &PythonCancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let task: SubmissionSpec =
            serde_json::from_str(&task_json).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let engine = self.inner.clone();
        let cancel = cancellation.token.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = engine.run(task, cancel).await.map_err(py_error)?;
            Ok(serde_json::to_string(&result).unwrap())
        })
    }

    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            engine.close().await.map_err(py_error)
        })
    }

    fn record_feedback<'py>(
        &self,
        py: Python<'py>,
        feedback_json: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let feedback: Feedback = serde_json::from_str(&feedback_json)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            engine.record_feedback(&feedback).await.map_err(py_error)
        })
    }
    fn evaluations<'py>(&self, py: Python<'py>, task_id: String) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Ok(
                serde_json::to_string(&engine.evaluations(&task_id).await.map_err(py_error)?)
                    .unwrap(),
            )
        })
    }
    fn metrics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Ok(serde_json::to_string(&engine.metrics().await.map_err(py_error)?).unwrap())
        })
    }
    fn recovery_records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let records = engine.recovery_records().await.map_err(py_error)?;
            serde_json::to_string(&records)
                .map_err(|_| PyRuntimeError::new_err("cannot serialize recovery records"))
        })
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PythonEngine>()?;
    m.add_class::<PythonSession>()?;
    m.add_class::<PythonCancellation>()
}
