//! Worker pool management for jsexec
//!
//! Manages a pool of workerd processes for executing JavaScript code.

mod manager;
mod worker;

pub use manager::{PoolConfig, PoolError, PoolManager, WorkerGuard};
pub use worker::{Worker, WorkerConfig};
