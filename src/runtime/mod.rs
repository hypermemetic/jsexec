//! JavaScript runtime utilities
//!
//! Contains the executor.js that runs inside workerd.

/// The executor JavaScript source code embedded at compile time
pub const EXECUTOR_JS: &str = include_str!("executor.js");
