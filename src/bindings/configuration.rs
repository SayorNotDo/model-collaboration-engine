//! Python exposes a read-only resolved configuration, not mutable connection dictionaries.
use super::py_error;
use crate::configuration::{self, ResolvedConfig};
use pyo3::prelude::*;

#[pyclass(name = "Configuration", frozen)]
pub(super) struct PythonConfig {
    pub(super) resolved: ResolvedConfig,
}

#[pymethods]
impl PythonConfig {
    #[staticmethod]
    fn load(path: String) -> PyResult<Self> {
        Ok(Self {
            resolved: configuration::load(path).map_err(py_error)?,
        })
    }

    #[staticmethod]
    fn parse(document: String, base_dir: String) -> PyResult<Self> {
        Ok(Self {
            resolved: configuration::parse(&document, base_dir).map_err(py_error)?,
        })
    }

    #[getter]
    fn database_path(&self) -> String {
        self.resolved.as_config().database_path.clone()
    }

    #[getter]
    fn model_ids(&self) -> Vec<String> {
        self.resolved
            .as_config()
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect()
    }
}
