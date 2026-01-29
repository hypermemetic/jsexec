//! Lambda function types and registry
//!
//! This module provides AWS Lambda-style function-as-a-service capabilities:
//! - Persistent function storage with SQLite
//! - Function versioning and aliases
//! - Handler pattern with event/context
//! - Per-function configuration and environment variables
//! - Async invocation modes

pub mod activation;
pub mod config;
pub mod context;
pub mod init;
pub mod invoker;
pub mod metrics;
pub mod pool_adapter;
pub mod registry;
pub mod runtime;
pub mod types;
pub mod versioning;

pub use activation::LambdaExec;
pub use config::LambdaConfig;
pub use context::LambdaContext;
pub use init::{run_migrations, LambdaSystem};
pub use invoker::{FunctionInvoker, InvocationMode};
pub use metrics::{GlobalStats, MetricsCollector};
pub use registry::{FunctionRegistry, ResolvedVersion};
pub use runtime::HandlerRuntime;
pub use types::*;
pub use versioning::VersioningManager;
