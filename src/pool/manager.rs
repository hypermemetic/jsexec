//! Pool manager for workerd Worker instances
//!
//! Manages a pool of workers with semaphore-based concurrency control,
//! automatic worker acquisition/release, and execution statistics.

use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tracing::{debug, error, info, instrument, warn};

use super::worker::{Worker, WorkerConfig};
use crate::types::{ExecutionContext, ExecutionLimits, JsExecEvent, PoolStatus};

/// Errors that can occur during pool operations
#[derive(Debug, Error)]
pub enum PoolError {
    #[error("Failed to spawn worker: {0}")]
    WorkerSpawn(String),

    #[error("No available workers in pool")]
    NoAvailableWorkers,

    #[error("Worker execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Pool is shutting down")]
    ShuttingDown,

    #[error("Worker health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Semaphore acquire failed: {0}")]
    SemaphoreAcquire(String),
}

/// Configuration for the worker pool
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Number of workers in the pool
    pub size: usize,
    /// Path to the workerd binary
    pub workerd_binary: String,
    /// Starting port number for workers (sequential assignment)
    pub worker_port_start: u16,
    /// Interval for health checks in milliseconds
    pub health_check_interval_ms: u64,
    /// Maximum number of executions before worker recycling
    pub max_worker_age_executions: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            size: 4,
            workerd_binary: "workerd".to_string(),
            worker_port_start: 8787,
            health_check_interval_ms: 5000,
            max_worker_age_executions: 1000,
        }
    }
}

/// Internal statistics for the pool
#[derive(Debug, Default)]
struct PoolStats {
    /// Total number of executions completed
    total_executions: u64,
    /// Number of failed executions
    failed_executions: u64,
    /// Total execution time in milliseconds
    total_execution_time_ms: u64,
}

impl PoolStats {
    fn record_execution(&mut self, duration_ms: u64, success: bool) {
        self.total_executions += 1;
        self.total_execution_time_ms += duration_ms;
        if !success {
            self.failed_executions += 1;
        }
    }

    fn average_execution_time_ms(&self) -> f64 {
        if self.total_executions == 0 {
            0.0
        } else {
            self.total_execution_time_ms as f64 / self.total_executions as f64
        }
    }
}

/// Manages a pool of workerd Worker instances
pub struct PoolManager {
    /// The workers in the pool
    workers: RwLock<Vec<Arc<Worker>>>,
    /// Semaphore for limiting concurrent access
    semaphore: Arc<Semaphore>,
    /// Pool configuration
    config: PoolConfig,
    /// Execution statistics
    stats: Arc<RwLock<PoolStats>>,
    /// Path to the executor JavaScript file
    executor_js_path: String,
}

impl PoolManager {
    /// Create a new pool manager with the given configuration
    ///
    /// Spawns `config.size` workers on sequential ports starting from
    /// `config.worker_port_start`.
    #[instrument(skip(config, executor_js_path), fields(pool_size = config.size))]
    pub async fn new(config: PoolConfig, executor_js_path: String) -> Result<Self, PoolError> {
        info!(
            size = config.size,
            port_start = config.worker_port_start,
            "Initializing worker pool"
        );

        let mut workers = Vec::with_capacity(config.size);

        // Create worker config
        let worker_config = WorkerConfig {
            workerd_binary: config.workerd_binary.clone().into(),
            base_port: config.worker_port_start,
            executor_js_path: Some(executor_js_path.clone().into()),
            ..Default::default()
        };

        for i in 0..config.size {
            let port = config.worker_port_start + i as u16;

            debug!(port = port, "Spawning worker");

            let worker = Worker::spawn(&worker_config, port)
                .await
                .map_err(|e| PoolError::WorkerSpawn(format!("port {}: {}", port, e)))?;

            workers.push(Arc::new(worker));
        }

        info!(count = workers.len(), "Worker pool initialized");

        Ok(Self {
            workers: RwLock::new(workers),
            semaphore: Arc::new(Semaphore::new(config.size)),
            config,
            stats: Arc::new(RwLock::new(PoolStats::default())),
            executor_js_path,
        })
    }

    /// Acquire a worker from the pool
    ///
    /// Returns a `WorkerGuard` that will release the worker when dropped.
    /// Blocks until a worker is available.
    #[instrument(skip(self))]
    pub async fn acquire(&self) -> Result<WorkerGuard, PoolError> {
        debug!("Acquiring worker from pool");

        // Acquire semaphore permit
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| PoolError::SemaphoreAcquire(e.to_string()))?;

        // Find an available worker
        let workers = self.workers.read().await;
        let worker = workers
            .iter()
            .find_map(|w| {
                if w.try_acquire() {
                    Some(Arc::clone(w))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                error!("Semaphore acquired but no workers available - this is a bug");
                PoolError::NoAvailableWorkers
            })?;

        debug!(worker_id = %worker.id(), "Worker acquired");

        Ok(WorkerGuard {
            worker,
            _permit: permit,
            pool_stats: Arc::clone(&self.stats),
            start_time: Instant::now(),
        })
    }

    /// Get the current status of the pool
    pub async fn status(&self) -> PoolStatus {
        let workers = self.workers.read().await;
        let stats = self.stats.read().await;

        let available_workers = workers.iter().filter(|w| w.is_available()).count();
        let busy_workers = workers.len() - available_workers;

        PoolStatus {
            total_workers: workers.len(),
            available_workers,
            busy_workers,
            total_executions: stats.total_executions,
            failed_executions: stats.failed_executions,
            average_execution_time_ms: stats.average_execution_time_ms(),
        }
    }

    /// Gracefully shutdown all workers in the pool
    #[instrument(skip(self))]
    pub async fn shutdown(&self) {
        info!("Shutting down worker pool");

        let workers = self.workers.read().await;

        for worker in workers.iter() {
            debug!(worker_id = %worker.id(), "Shutting down worker");
            if let Err(e) = worker.shutdown().await {
                warn!(worker_id = %worker.id(), error = %e, "Error shutting down worker");
            }
        }

        info!("Worker pool shutdown complete");
    }

    /// Get the pool configuration
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Get the executor JavaScript path
    pub fn executor_js_path(&self) -> &str {
        &self.executor_js_path
    }
}

/// Guard that holds a worker and releases it when dropped
///
/// This struct provides access to a worker from the pool and ensures
/// the worker is properly released when the guard is dropped.
pub struct WorkerGuard {
    /// The acquired worker
    worker: Arc<Worker>,
    /// Semaphore permit (released on drop)
    _permit: OwnedSemaphorePermit,
    /// Reference to pool stats for recording execution metrics
    pool_stats: Arc<RwLock<PoolStats>>,
    /// When this guard was created
    start_time: Instant,
}

impl WorkerGuard {
    /// Get the worker ID
    pub fn id(&self) -> &str {
        self.worker.id()
    }

    /// Execute JavaScript code on the worker
    ///
    /// Returns a stream of execution events including console output,
    /// return values, and errors.
    pub fn execute(
        &self,
        code: String,
        context: ExecutionContext,
    ) -> impl Stream<Item = JsExecEvent> + '_ {
        let limits = ExecutionLimits::default();
        self.worker.execute(code, limits, Some(context), None)
    }

    /// Record execution completion
    ///
    /// Call this when execution completes to update pool statistics.
    pub async fn record_completion(&self, success: bool) {
        let duration_ms = self.start_time.elapsed().as_millis() as u64;
        let mut stats = self.pool_stats.write().await;
        stats.record_execution(duration_ms, success);
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        // Release the worker back to the pool
        self.worker.release();
        debug!(worker_id = %self.worker.id(), "Worker released back to pool");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.size, 4);
        assert_eq!(config.worker_port_start, 8787);
        assert_eq!(config.health_check_interval_ms, 5000);
        assert_eq!(config.max_worker_age_executions, 1000);
    }

    #[test]
    fn test_pool_stats_recording() {
        let mut stats = PoolStats::default();

        stats.record_execution(100, true);
        assert_eq!(stats.total_executions, 1);
        assert_eq!(stats.failed_executions, 0);
        assert_eq!(stats.total_execution_time_ms, 100);
        assert_eq!(stats.average_execution_time_ms(), 100.0);

        stats.record_execution(200, false);
        assert_eq!(stats.total_executions, 2);
        assert_eq!(stats.failed_executions, 1);
        assert_eq!(stats.total_execution_time_ms, 300);
        assert_eq!(stats.average_execution_time_ms(), 150.0);
    }

    #[test]
    fn test_pool_stats_average_with_no_executions() {
        let stats = PoolStats::default();
        assert_eq!(stats.average_execution_time_ms(), 0.0);
    }
}
