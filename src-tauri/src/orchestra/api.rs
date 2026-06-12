//! OwnAgent REST client — calls localhost API endpoints.

use serde_json::Value;

use crate::error::AppError;

/// Client for the OwnAgent local REST API.
pub struct OwnAgentApi {
    base_url: String,
}

impl OwnAgentApi {
    pub fn new(port: u16) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{}", port),
        }
    }

    /// List all flows.
    pub async fn list_flows(&self) -> Result<Value, AppError> {
        let url = format!("{}/api/flows", self.base_url);
        self.get(&url).await
    }

    /// Create a new flow run.
    pub async fn create_run(&self, flow_id: &str, input: Value) -> Result<Value, AppError> {
        let url = format!("{}/api/flows/{}/runs", self.base_url, flow_id);
        self.post(&url, input).await
    }

    /// Query the status of a run.
    pub async fn get_run_status(&self, run_id: &str) -> Result<Value, AppError> {
        let url = format!("{}/api/runs/{}", self.base_url, run_id);
        self.get(&url).await
    }

    async fn get(&self, url: &str) -> Result<Value, AppError> {
        // Skeleton: actual reqwest calls will be implemented when async runtime is wired up.
        Err(AppError::Orchestra(format!("GET {} not yet implemented (skeleton)", url)))
    }

    async fn post(&self, url: &str, _body: Value) -> Result<Value, AppError> {
        // Skeleton: actual reqwest calls will be implemented when async runtime is wired up.
        Err(AppError::Orchestra(format!("POST {} not yet implemented (skeleton)", url)))
    }
}
