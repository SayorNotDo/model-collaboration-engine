//! JSON boundary keeps the Python API independent of provider SDK types.
use crate::{contracts::*, engine::Engine};
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn py_error(error: EngineError) -> PyErr {
    PyRuntimeError::new_err(serde_json::to_string(&error).unwrap())
}

#[pyclass(name = "Engine")]
struct PythonEngine {
    inner: Arc<Engine>,
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

#[pymethods]
impl PythonEngine {
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
        let task: TaskSpec =
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
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PythonEngine>()?;
    m.add_class::<PythonCancellation>()
}
