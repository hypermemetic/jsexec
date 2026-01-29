//! Lambda system configuration

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the Lambda system
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LambdaConfig {
    /// Path to SQLite database file
    /// If not absolute, relative to working directory
    /// Use ":memory:" for in-memory database (testing only)
    pub database_url: String,

    /// Enable worker pooling for warm starts
    /// If false, uses ephemeral execution (fresh process per invocation)
    #[serde(default)]
    pub enable_pool: bool,

    /// Worker pool size (only used if enable_pool = true)
    /// Defaults to number of CPU cores
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_size: Option<usize>,

    /// Default timeout for functions (milliseconds)
    /// Can be overridden per-function
    #[serde(default = "default_timeout")]
    pub default_timeout_ms: u64,

    /// Default memory limit for functions (MB)
    /// Can be overridden per-function
    #[serde(default = "default_memory")]
    pub default_memory_mb: u64,

    /// Enable metrics collection
    #[serde(default = "default_true")]
    pub enable_metrics: bool,

    /// Path to workerd binary
    /// Defaults to "workerd" (searches PATH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workerd_path: Option<PathBuf>,

    /// Path to esbuild binary for bundling
    /// Defaults to "esbuild" (searches PATH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub esbuild_path: Option<PathBuf>,
}

fn default_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_memory() -> u64 {
    128 // 128 MB
}

fn default_true() -> bool {
    true
}

impl Default for LambdaConfig {
    fn default() -> Self {
        Self {
            database_url: "sqlite:lambda.db".to_string(),
            enable_pool: false,
            pool_size: None,
            default_timeout_ms: default_timeout(),
            default_memory_mb: default_memory(),
            enable_metrics: true,
            workerd_path: None,
            esbuild_path: None,
        }
    }
}

impl LambdaConfig {
    /// Create a new config with the given database URL
    pub fn new(database_url: impl Into<String>) -> Self {
        Self {
            database_url: database_url.into(),
            ..Default::default()
        }
    }

    /// Create an in-memory config (for testing)
    pub fn in_memory() -> Self {
        Self {
            database_url: "sqlite::memory:".to_string(),
            ..Default::default()
        }
    }

    /// Enable worker pooling
    pub fn with_pool(mut self, pool_size: usize) -> Self {
        self.enable_pool = true;
        self.pool_size = Some(pool_size);
        self
    }

    /// Set default timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.default_timeout_ms = timeout_ms;
        self
    }

    /// Set default memory
    pub fn with_memory(mut self, memory_mb: u64) -> Self {
        self.default_memory_mb = memory_mb;
        self
    }

    /// Disable metrics collection
    pub fn without_metrics(mut self) -> Self {
        self.enable_metrics = false;
        self
    }
}
