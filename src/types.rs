//! Type definitions for the jsexec plugin
//!
//! This module contains all the core types used throughout the jsexec system
//! for JavaScript execution, resource tracking, and event handling.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

// =============================================================================
// Newtypes
// =============================================================================

/// Unique identifier for a stored script
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ScriptId(pub Uuid);

impl ScriptId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ScriptId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ScriptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a Lambda function (re-exported from lambda module)
pub use crate::lambda::types::FunctionId;

/// Unique identifier for a single execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ExecutionId(pub Uuid);

impl ExecutionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =============================================================================
// Resource Tracking
// =============================================================================

/// Metrics collected during script execution
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResourceMetrics {
    /// CPU time consumed in milliseconds
    pub cpu_time_ms: u64,
    /// Wall clock time elapsed in milliseconds
    pub wall_time_ms: u64,
    /// Current memory usage in bytes
    pub memory_used_bytes: u64,
    /// Peak memory usage in bytes
    pub memory_peak_bytes: u64,
}

/// Limits applied to script execution
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionLimits {
    /// Maximum CPU time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<u64>,
    /// Maximum wall clock time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_time_ms: Option<u64>,
    /// Maximum memory usage in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Maximum number of fetch requests allowed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fetch_requests: Option<u32>,
    /// Maximum total response bytes from fetch requests
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
}

// =============================================================================
// Source Location & Stack Traces
// =============================================================================

/// Location within a script file
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceLocation {
    /// Line number (1-indexed)
    pub line: u32,
    /// Column number (1-indexed)
    pub column: u32,
    /// Filename if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// A single frame in a stack trace
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StackFrame {
    /// Function name if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    /// Source location of this frame
    pub location: SourceLocation,
}

// =============================================================================
// Logging
// =============================================================================

/// Log level for console output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
    Trace,
}

// =============================================================================
// Events
// =============================================================================

/// Events emitted during JavaScript execution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsExecEvent {
    /// Execution has started
    ExecutionStarted {
        execution_id: ExecutionId,
        #[serde(skip_serializing_if = "Option::is_none")]
        script_id: Option<ScriptId>,
        worker_id: String,
    },

    /// Execution has completed successfully
    ExecutionCompleted {
        execution_id: ExecutionId,
        metrics: ResourceMetrics,
    },

    /// Console output from the script
    Console {
        level: LogLevel,
        args: Vec<Value>,
        timestamp_ms: u64,
    },

    /// Script returned a value
    Returned { value: Value },

    /// A fetch request has started
    FetchStarted {
        request_id: String,
        method: String,
        url: String,
    },

    /// A fetch request has completed
    FetchCompleted {
        request_id: String,
        status: u16,
        duration_ms: u64,
    },

    /// An error occurred during execution
    Error {
        message: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<SourceLocation>,
        stack: Vec<StackFrame>,
    },

    /// Resource usage is approaching limits
    ResourceWarning {
        resource: String,
        current: u64,
        limit: u64,
        message: String,
    },

    /// A script has been stored
    ScriptStored {
        script_id: ScriptId,
        name: String,
        size_bytes: u64,
        hash: String,
    },

    /// A script has been deleted
    ScriptDeleted { script_id: ScriptId },
}

// =============================================================================
// Script Management
// =============================================================================

/// Metadata about a stored script
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScriptMetadata {
    /// Unique identifier for this script
    pub id: ScriptId,
    /// Human-readable name
    pub name: String,
    /// Size of the script in bytes
    pub size_bytes: u64,
    /// SHA-256 hash of the script content
    pub hash: String,
    /// When the script was created (Unix timestamp ms)
    pub created_at: u64,
    /// When the script was last modified (Unix timestamp ms)
    pub updated_at: u64,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tags for organization
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

// =============================================================================
// Bindings
// =============================================================================

/// Configuration for bindings available to scripts
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BindingConfig {
    /// Key-value store binding
    Kv {
        /// Name of the binding in JavaScript
        name: String,
        /// Namespace for the KV store
        namespace: String,
    },

    /// Environment variable binding
    Var {
        /// Name of the binding in JavaScript
        name: String,
        /// Value of the variable
        value: String,
    },

    /// Secret binding (value not logged)
    Secret {
        /// Name of the binding in JavaScript
        name: String,
        /// Secret value
        value: String,
    },

    /// Plexus API binding
    Plexus {
        /// Name of the binding in JavaScript
        name: String,
        /// Methods to expose
        #[serde(default)]
        methods: Vec<String>,
    },

    /// Fetch API binding with optional restrictions
    Fetch {
        /// Name of the binding in JavaScript
        name: String,
        /// Allowed URL patterns (glob)
        #[serde(default)]
        allowed_hosts: Vec<String>,
    },
}

// =============================================================================
// Execution Context
// =============================================================================

/// Context passed to script execution
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionContext {
    /// Arbitrary data available as `ctx.data` in the script
    #[serde(default)]
    pub data: Value,
    /// Environment variables available as `ctx.env`
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional request context for HTTP-triggered executions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestContext>,
}

/// HTTP request context for request-triggered executions
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestContext {
    /// HTTP method
    pub method: String,
    /// Request URL
    pub url: String,
    /// Request headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Request body if present
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

// =============================================================================
// Module Configuration
// =============================================================================

/// Configuration for a preloaded JavaScript module
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModuleConfig {
    /// Module name used for imports (e.g., "lodash", "utils")
    pub name: String,
    /// Module source - either inline code or a file path
    #[serde(flatten)]
    pub source: ModuleSource,
    /// Whether to expose the module's default export on globalThis
    /// If true, `globalThis.{name}` will be set to the module's default export
    #[serde(default)]
    pub expose_global: bool,
}

/// Source of a module's code
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModuleSource {
    /// Inline JavaScript code
    Code(String),
    /// Path to a JavaScript file (read at startup)
    Path(std::path::PathBuf),
}

impl ModuleConfig {
    /// Create a new module config with inline code
    pub fn inline(name: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: ModuleSource::Code(code.into()),
            expose_global: false,
        }
    }

    /// Create a new module config from a file path with explicit name
    pub fn from_file(name: impl Into<String>, path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            name: name.into(),
            source: ModuleSource::Path(path.into()),
            expose_global: false,
        }
    }

    /// Create a new module config from a file path, deriving name from filename
    /// Name is derived by taking the filename without extension, with dashes converted to underscores
    /// Example: "/path/to/lodash.js" → name: "lodash"
    /// Example: "/path/to/tools-simple.js" → name: "tools_simple"
    pub fn from_path(path: impl Into<std::path::PathBuf>) -> Self {
        let path_buf = path.into();
        let name = path_buf
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("module")
            .replace('-', "_"); // Convert dashes to underscores for valid JS identifiers

        Self {
            name,
            source: ModuleSource::Path(path_buf),
            expose_global: false,
        }
    }

    /// Set whether to expose on globalThis
    pub fn with_global(mut self, expose: bool) -> Self {
        self.expose_global = expose;
        self
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Local library configuration for bundling
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LocalLibrary {
    /// Name to import the library as
    pub name: String,
    /// Path to the entry point (can be .ts or .js)
    pub entry_point: std::path::PathBuf,
    /// Whether to run type checking (requires tsc)
    #[serde(default)]
    pub typecheck: bool,
}

/// Configuration for the jsexec system
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JsExecConfig {
    /// Default execution limits applied to all executions
    #[serde(default)]
    pub default_limits: ExecutionLimits,
    /// Enable console output capture
    #[serde(default = "default_true")]
    pub capture_console: bool,
    /// Preloaded modules available to all executions
    #[serde(default)]
    pub modules: Vec<ModuleConfig>,
    /// Path to node_modules directory for bundling npm packages
    /// When set along with npm_packages, those packages will be auto-bundled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_modules_path: Option<std::path::PathBuf>,
    /// List of npm package names to auto-bundle and make available
    /// Packages are bundled from node_modules_path using esbuild
    /// Example: vec!["typescript", "lodash", "@types/node"]
    #[serde(default)]
    pub npm_packages: Vec<String>,
    /// Local libraries to bundle and load (supports TypeScript)
    /// Libraries are bundled with esbuild and optionally type-checked with tsc
    #[serde(default)]
    pub local_libraries: Vec<LocalLibrary>,
    /// Path to esbuild binary for bundling npm packages
    /// Defaults to "esbuild" (searches PATH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub esbuild_path: Option<std::path::PathBuf>,
    /// Path to tsc binary for type checking
    /// Defaults to "tsc" (searches PATH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tsc_path: Option<std::path::PathBuf>,
}

fn default_true() -> bool {
    true
}

impl Default for JsExecConfig {
    fn default() -> Self {
        Self {
            default_limits: ExecutionLimits::default(),
            capture_console: true,
            modules: Vec::new(),
            node_modules_path: None,
            npm_packages: Vec::new(),
            local_libraries: Vec::new(),
            esbuild_path: None,
            tsc_path: None,
        }
    }
}

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur during worker operations
#[derive(Debug, Clone, Error, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerError {
    /// Script compilation failed
    #[error("compilation error: {message}")]
    Compilation { message: String },

    /// Script execution failed
    #[error("execution error: {message}")]
    Execution { message: String },

    /// Script exceeded resource limits
    #[error("resource limit exceeded: {resource} ({current} > {limit})")]
    ResourceLimit {
        resource: String,
        current: u64,
        limit: u64,
    },

    /// Script execution timed out
    #[error("execution timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Worker is in an invalid state
    #[error("invalid worker state: {message}")]
    InvalidState { message: String },

    /// Communication with worker failed
    #[error("worker communication error: {message}")]
    Communication { message: String },

    /// Script not found
    #[error("script not found: {script_id}")]
    ScriptNotFound { script_id: ScriptId },

    /// Binding error
    #[error("binding error: {message}")]
    Binding { message: String },
}

/// Errors that can occur during pool operations
#[derive(Debug, Clone, Error, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PoolError {
    /// Failed to create a new worker
    #[error("failed to create worker: {message}")]
    WorkerCreation { message: String },

    /// Pool is at capacity and cannot accept more work
    #[error("pool at capacity ({max_workers} workers)")]
    AtCapacity { max_workers: usize },

    /// Pool has been shut down
    #[error("pool has been shut down")]
    Shutdown,

    /// Worker error during execution
    #[error("worker error: {0}")]
    Worker(#[from] WorkerError),

    /// Internal pool error
    #[error("internal pool error: {message}")]
    Internal { message: String },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_id_serialization() {
        let id = ScriptId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: ScriptId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_execution_id_serialization() {
        let id = ExecutionId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: ExecutionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_event_serialization() {
        let event = JsExecEvent::Console {
            level: LogLevel::Info,
            args: vec![Value::String("hello".to_string())],
            timestamp_ms: 12345,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"console\""));
        assert!(json.contains("\"level\":\"info\""));
    }

    #[test]
    fn test_binding_config_serialization() {
        let binding = BindingConfig::Kv {
            name: "store".to_string(),
            namespace: "default".to_string(),
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(json.contains("\"type\":\"kv\""));
    }

    #[test]
    fn test_worker_error_display() {
        let err = WorkerError::Timeout { timeout_ms: 5000 };
        assert_eq!(err.to_string(), "execution timed out after 5000ms");
    }

    #[test]
    fn test_pool_error_from_worker_error() {
        let worker_err = WorkerError::Compilation {
            message: "syntax error".to_string(),
        };
        let pool_err: PoolError = worker_err.into();
        assert!(matches!(pool_err, PoolError::Worker(_)));
    }

    #[test]
    fn test_config_defaults() {
        let config = JsExecConfig::default();
        assert!(config.capture_console);
        // default_limits should be empty (no limits set)
        assert!(config.default_limits.cpu_time_ms.is_none());
    }

    #[test]
    fn test_module_from_path() {
        let module = ModuleConfig::from_path("/path/to/lodash.js");
        assert_eq!(module.name, "lodash");
        assert!(!module.expose_global);
        assert!(matches!(module.source, ModuleSource::Path(_)));
    }

    #[test]
    fn test_module_from_path_with_global() {
        let module = ModuleConfig::from_path("/path/to/utils.js").with_global(true);
        assert_eq!(module.name, "utils");
        assert!(module.expose_global);
    }

    #[test]
    fn test_module_from_path_no_extension() {
        let module = ModuleConfig::from_path("/path/to/module");
        assert_eq!(module.name, "module");
    }
}
