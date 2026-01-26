//! Worker implementation for jsexec
//!
//! Wraps a single workerd process and provides methods for executing JavaScript code.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_stream::stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::types::{BindingConfig, ExecutionContext, ExecutionLimits, JsExecEvent};

/// Error type for worker operations
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("Failed to spawn workerd process: {0}")]
    SpawnFailed(#[from] std::io::Error),

    #[error("Health check failed after {attempts} attempts")]
    HealthCheckFailed { attempts: u32 },

    #[error("Failed to create config file: {0}")]
    ConfigError(String),

    #[error("Worker process exited unexpectedly")]
    ProcessExited,

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Failed to parse response: {0}")]
    ParseError(String),

    #[error("Worker is busy")]
    Busy,
}

/// Configuration for spawning workers
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Path to the workerd binary
    pub workerd_binary: PathBuf,
    /// Base port for worker allocation
    pub base_port: u16,
    /// Path to the executor.js file (optional - will use embedded if not provided)
    pub executor_js_path: Option<PathBuf>,
    /// Timeout for health check attempts
    pub health_check_timeout: Duration,
    /// Maximum number of health check attempts
    pub health_check_attempts: u32,
    /// Delay between health check attempts
    pub health_check_delay: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            workerd_binary: PathBuf::from("workerd"),
            base_port: 8787,
            executor_js_path: None,
            health_check_timeout: Duration::from_secs(1),
            health_check_attempts: 30,
            health_check_delay: Duration::from_millis(100),
        }
    }
}

/// A single workerd process wrapper
pub struct Worker {
    /// Unique identifier for this worker
    id: String,
    /// The workerd child process
    process: RwLock<Child>,
    /// Port this worker is listening on
    port: u16,
    /// Whether the worker is currently executing a request
    busy: AtomicBool,
    /// Number of executions this worker has performed
    execution_count: AtomicU64,
    /// HTTP client for communicating with the worker
    client: Client,
    /// Temporary directory holding config files (kept alive for process lifetime)
    _temp_dir: tempfile::TempDir,
}

impl Worker {
    /// Spawn a new workerd process
    ///
    /// This creates the necessary config files, spawns the workerd process,
    /// and waits for it to become healthy before returning.
    pub async fn spawn(config: &WorkerConfig, port: u16) -> Result<Self, WorkerError> {
        let id = Uuid::new_v4().to_string();
        info!(worker_id = %id, port = port, "Spawning new worker");

        // Create temporary directory for config files
        let temp_dir = tempfile::tempdir()?;
        let config_path = temp_dir.path().join("config.capnp");
        let executor_path = temp_dir.path().join("executor.js");

        // Write executor.js
        let executor_content = if let Some(ref path) = config.executor_js_path {
            tokio::fs::read_to_string(path).await?
        } else {
            crate::runtime::EXECUTOR_JS.to_string()
        };
        tokio::fs::write(&executor_path, &executor_content).await?;

        // Generate and write workerd config
        // Use relative path since config and executor.js are in the same directory
        let workerd_config = generate_workerd_config(port, "executor.js");
        tokio::fs::write(&config_path, &workerd_config).await?;

        debug!(
            worker_id = %id,
            config_path = %config_path.display(),
            "Generated workerd config"
        );

        // Spawn workerd process
        let process = Command::new(&config.workerd_binary)
            .arg("serve")
            .arg(&config_path)
            .arg("--verbose")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let client = Client::builder()
            .timeout(Duration::from_secs(300)) // 5 minute timeout for long-running scripts
            .build()?;

        let worker = Self {
            id,
            process: RwLock::new(process),
            port,
            busy: AtomicBool::new(false),
            execution_count: AtomicU64::new(0),
            client,
            _temp_dir: temp_dir,
        };

        // Wait for health check
        worker.wait_for_healthy(config).await?;

        info!(worker_id = %worker.id, port = port, "Worker is ready");
        Ok(worker)
    }

    /// Wait for the worker to become healthy
    async fn wait_for_healthy(&self, config: &WorkerConfig) -> Result<(), WorkerError> {
        for attempt in 1..=config.health_check_attempts {
            trace!(
                worker_id = %self.id,
                attempt = attempt,
                max_attempts = config.health_check_attempts,
                "Health check attempt"
            );

            if self.health_check().await {
                debug!(worker_id = %self.id, "Health check passed");
                return Ok(());
            }

            // Check if process is still running
            {
                let mut proc = self.process.write().await;
                if let Ok(Some(status)) = proc.try_wait() {
                    error!(
                        worker_id = %self.id,
                        exit_status = ?status,
                        "Worker process exited during startup"
                    );
                    return Err(WorkerError::ProcessExited);
                }
            }

            tokio::time::sleep(config.health_check_delay).await;
        }

        Err(WorkerError::HealthCheckFailed {
            attempts: config.health_check_attempts,
        })
    }

    /// Get the worker's unique identifier
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the port this worker is listening on
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Try to acquire this worker for execution
    ///
    /// Returns true if the worker was successfully acquired (was not busy).
    /// Uses compare-and-swap to ensure thread safety.
    pub fn try_acquire(&self) -> bool {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Release this worker after execution
    pub fn release(&self) {
        self.busy.store(false, Ordering::SeqCst);
    }

    /// Check if this worker is available for execution
    pub fn is_available(&self) -> bool {
        !self.busy.load(Ordering::SeqCst)
    }

    /// Get the number of executions this worker has performed
    pub fn execution_count(&self) -> u64 {
        self.execution_count.load(Ordering::SeqCst)
    }

    /// Execute JavaScript code and stream the results
    ///
    /// This POSTs to the worker's /execute endpoint and streams back
    /// NDJSON events as they arrive.
    pub fn execute(
        &self,
        code: String,
        limits: ExecutionLimits,
        context: Option<ExecutionContext>,
        bindings: Option<Vec<BindingConfig>>,
    ) -> impl Stream<Item = JsExecEvent> + '_ {
        stream! {
            // Increment execution count
            self.execution_count.fetch_add(1, Ordering::SeqCst);

            let url = format!("http://127.0.0.1:{}/execute", self.port);

            // Build request body
            let body = ExecuteRequest {
                code,
                limits,
                context,
                bindings,
            };

            debug!(
                worker_id = %self.id,
                url = %url,
                "Sending execute request"
            );

            // Send request
            let response = match self.client
                .post(&url)
                .json(&body)
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    error!(worker_id = %self.id, error = %e, "Failed to send execute request");
                    yield JsExecEvent::Error {
                        message: format!("Failed to send request: {}", e),
                        name: "NetworkError".to_string(),
                        location: None,
                        stack: Vec::new(),
                    };
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!(
                    worker_id = %self.id,
                    status = %status,
                    body = %body,
                    "Execute request failed"
                );
                yield JsExecEvent::Error {
                    message: format!("HTTP {}: {}", status, body),
                    name: "HttpError".to_string(),
                    location: None,
                    stack: Vec::new(),
                };
                return;
            }

            // Stream the response as NDJSON
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        error!(worker_id = %self.id, error = %e, "Error reading response stream");
                        yield JsExecEvent::Error {
                            message: format!("Stream read error: {}", e),
                            name: "StreamError".to_string(),
                            location: None,
                            stack: Vec::new(),
                        };
                        return;
                    }
                };

                // Append chunk to buffer
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim();

                    if !line.is_empty() {
                        match serde_json::from_str::<JsExecEvent>(line) {
                            Ok(event) => {
                                trace!(worker_id = %self.id, event = ?event, "Received event");
                                yield event;
                            }
                            Err(e) => {
                                warn!(
                                    worker_id = %self.id,
                                    line = %line,
                                    error = %e,
                                    "Failed to parse NDJSON line"
                                );
                            }
                        }
                    }

                    buffer = buffer[newline_pos + 1..].to_string();
                }
            }

            // Process any remaining data in buffer
            let remaining = buffer.trim();
            if !remaining.is_empty() {
                match serde_json::from_str::<JsExecEvent>(remaining) {
                    Ok(event) => {
                        trace!(worker_id = %self.id, event = ?event, "Received final event");
                        yield event;
                    }
                    Err(e) => {
                        warn!(
                            worker_id = %self.id,
                            line = %remaining,
                            error = %e,
                            "Failed to parse final NDJSON line"
                        );
                    }
                }
            }
        }
    }

    /// Perform a health check against this worker
    async fn health_check(&self) -> bool {
        let url = format!("http://127.0.0.1:{}/health", self.port);

        match self
            .client
            .get(&url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Shutdown this worker gracefully
    pub async fn shutdown(&self) -> Result<(), WorkerError> {
        info!(worker_id = %self.id, "Shutting down worker");

        let mut process = self.process.write().await;

        // Kill the process
        if let Err(e) = process.kill().await {
            warn!(worker_id = %self.id, error = %e, "Error killing worker process");
        }

        // Wait for it to exit
        match process.wait().await {
            Ok(status) => {
                debug!(worker_id = %self.id, status = ?status, "Worker exited");
            }
            Err(e) => {
                warn!(worker_id = %self.id, error = %e, "Error waiting for worker");
            }
        }

        Ok(())
    }
}

/// Request body for /execute endpoint
#[derive(Debug, Serialize, Deserialize)]
struct ExecuteRequest {
    code: String,
    limits: ExecutionLimits,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<ExecutionContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bindings: Option<Vec<BindingConfig>>,
}

/// Generate a Cap'n Proto configuration file for workerd
///
/// This creates a minimal configuration that:
/// - Sets up a single worker with our executor.js
/// - Binds to the specified port on localhost
/// - Enables the necessary APIs for JavaScript execution
fn generate_workerd_config(port: u16, executor_js_path: &str) -> String {
    format!(
        r#"using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [
    (name = "jsexec", worker = .jsexecWorker),
  ],

  sockets = [
    (
      name = "http",
      address = "127.0.0.1:{port}",
      http = (),
      service = "jsexec"
    ),
  ]
);

const jsexecWorker :Workerd.Worker = (
  modules = [
    (name = "worker", esModule = embed "{executor_js_path}"),
  ],
  compatibilityDate = "2024-01-01",
  compatibilityFlags = ["nodejs_compat"],
);
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_workerd_config() {
        let config = generate_workerd_config(8787, "/tmp/executor.js");
        assert!(config.contains("127.0.0.1:8787"));
        assert!(config.contains("/tmp/executor.js"));
        assert!(config.contains("jsexec"));
    }

    #[test]
    fn test_worker_config_default() {
        let config = WorkerConfig::default();
        assert_eq!(config.base_port, 8787);
        assert_eq!(config.health_check_attempts, 30);
    }
}
