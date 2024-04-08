#![allow(clippy::too_many_arguments)]
#![allow(non_local_definitions)] // Try removing after PyO3 upgrade

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use locustdb_compression_utils::column::{Column, Mixed};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::logging_client::LoggingClient;

lazy_static! {
    static ref RT: tokio::runtime::Runtime = tokio::runtime::Runtime::new().unwrap();
    static ref DEFAULT_CLIENT: Arc<Mutex<LoggingClient>> = {
        let _guard = RT.enter();
        Arc::new(Mutex::new(LoggingClient::new(
            std::time::Duration::from_secs(1),
            "http://localhost:8080",
            128 * (1 << 20),
        )))
    };
}

#[pyclass]
struct Client {
    client: LoggingClient,
}

#[pymodule]
fn locustdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
    env_logger::init();
    m.add_class::<Client>()?;
    Ok(())
}

#[pymethods]
impl Client {
    #[new]
    fn new(url: &str) -> Self {
        let _guard = RT.enter();
        Self {
            client: LoggingClient::new(std::time::Duration::from_secs(1), url, 128 * (1 << 20)),
        }
    }

    fn log(&mut self, table: &str, metrics: HashMap<String, f64>) -> PyResult<()> {
        self.client.log(table, metrics);
        Ok(())
    }

    fn multi_query(&self, py: Python, queries: Vec<String>) -> PyResult<PyObject> {
        let results = RT
            .block_on(self.client.multi_query(queries))
            .map_err(|e| PyErr::new::<PyException, _>(format!("{:?}", e)))?;
        let py_result = PyList::new_bound(
            py,
            results.into_iter().map(|result| {
                let columns = PyDict::new_bound(py);
                for (key, value) in result.columns {
                    columns.set_item(key, column_to_python(py, value)).unwrap();
                }
                columns
            }),
        );
        Ok(py_result.into_py(py))
    }

    fn query(&self, py: Python, query: String) -> PyResult<PyObject> {
        let result = RT
            .block_on(self.client.multi_query(vec![query]))
            .map_err(|e| PyErr::new::<PyException, _>(format!("{:?}", e)))?;
        assert_eq!(result.len(), 1);
        let columns = PyDict::new_bound(py);
        for (key, value) in result.into_iter().next().unwrap().columns {
            columns.set_item(key, column_to_python(py, value)).unwrap();
        }
        Ok(columns.into_py(py))
    }

    #[pyo3(signature = (table, pattern = None))]
    fn columns(&self, py: Python, table: String, pattern: Option<String>) -> PyResult<PyObject> {
        let response = RT
            .block_on(self.client.columns(table, pattern))
            .map_err(|e| PyErr::new::<PyException, _>(format!("{:?}", e)))?;
        Ok(response.columns.into_py(py))
    }
}

fn column_to_python(py: Python, column: Column) -> PyObject {
    match column {
        Column::Float(xs) => xs.into_py(py),
        Column::Int(xs) => xs.into_py(py),
        Column::String(xs) => xs.into_py(py),
        Column::Mixed(xs) => PyList::new_bound(py, xs.iter().map(|x| mixed_to_python(py, x))).into_py(py),
        Column::Null(n) => n.into_py(py),
        Column::Xor(xs) => xs.into_py(py),
    }
}

fn mixed_to_python(py: Python, value: &Mixed) -> PyObject {
    match value {
        Mixed::Int(i) => i.into_py(py),
        Mixed::Float(f) => f.into_py(py),
        Mixed::Str(s) => s.into_py(py),
        Mixed::Null => None::<()>.into_py(py),
    }
}
