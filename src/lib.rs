//! JsExec - JavaScript Execution Plugin for Plexus
//!
//! This crate provides a Plexus activation that executes JavaScript code
//! in sandboxed V8 isolates using Cloudflare's workerd runtime.
//!
//! # Architecture
//!
//! Each execution spawns a fresh workerd process with the user's code
//! embedded directly as the worker module. This provides:
//! - True isolation between executions
//! - No need for eval/dynamic code generation
//! - Clean teardown after each execution
//!
//! # Example
//!
//! ```rust,no_run
//! use jsexec::{JsExec, JsExecConfig};
//! use plexus_core::plexus::Plexus;
//! use std::sync::Arc;
//!
//! # async fn example() {
//! let jsexec = JsExec::new(JsExecConfig::default());
//! let plexus = Arc::new(Plexus::new().register(jsexec));
//! # }
//! ```

pub mod activation;
pub mod bundler;
pub mod import_transform;
pub mod lambda;
pub mod plexus_env;
pub mod runner;
pub mod serde_helpers;
pub mod types;

pub use activation::JsExec;
pub use bundler::{bundle_package, bundle_package_cached, BundleCache, BundleConfig, BundleError, BundleFormat, NoCache};
pub use runner::{execute, RunnerConfig, RunnerError};
pub use types::{
    BindingConfig, ExecutionContext, ExecutionLimits, JsExecConfig, JsExecEvent,
    LocalLibrary, ModuleConfig, ModuleSource, ResourceMetrics, ScriptId, ScriptMetadata,
};
