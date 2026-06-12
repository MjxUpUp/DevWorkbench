//! OwnAgent Python sidecar lifecycle management.

use std::process::Child;
use std::sync::Mutex;

use crate::error::AppError;

/// Manages the OwnAgent Python sidecar process.
pub struct OwnAgentSidecar {
    process: Mutex<Option<Child>>,
    port: u16,
}

impl OwnAgentSidecar {
    pub fn new(port: u16) -> Self {
        Self {
            process: Mutex::new(None),
            port,
        }
    }

    /// Discover Python and start the uvicorn server.
    pub fn start(&self) -> Result<(), AppError> {
        let mut guard = self.process.lock().map_err(|e| AppError::Orchestra(format!("Lock error: {}", e)))?;
        if guard.is_some() {
            return Err(AppError::Orchestra("Sidecar already running".into()));
        }

        let python = which::which("python3")
            .or_else(|_| which::which("python"))
            .map_err(|_| AppError::Orchestra("Python not found".into()))?;

        use std::process::{Command, Stdio};
        let child = Command::new(python)
            .args(["-m", "uvicorn", "ownagent.main:app", "--host", "127.0.0.1", "--port", &self.port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Orchestra(format!("Failed to start sidecar: {}", e)))?;

        *guard = Some(child);
        log::info!("OwnAgent sidecar started on port {}", self.port);
        Ok(())
    }

    /// Gracefully stop the sidecar process.
    pub fn stop(&self) -> Result<(), AppError> {
        let mut guard = self.process.lock().map_err(|e| AppError::Orchestra(format!("Lock error: {}", e)))?;
        if let Some(mut child) = guard.take() {
            child.kill().map_err(|e| AppError::Orchestra(format!("Failed to kill sidecar: {}", e)))?;
            log::info!("OwnAgent sidecar stopped");
        }
        Ok(())
    }

    /// Check if the sidecar process is still running.
    pub fn is_running(&self) -> bool {
        let mut guard = match self.process.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if let Some(child) = guard.as_mut() {
            // Try non-blocking wait to check if still alive
            match child.try_wait() {
                Ok(None) => true,   // Still running
                Ok(Some(_)) => false, // Exited
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}
