//! Lambda-specific type definitions

use crate::types::{JsExecEvent, ResourceMetrics};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

// =============================================================================
// Newtypes
// =============================================================================

/// Unique identifier for a Lambda function
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct FunctionId(pub Uuid);

impl FunctionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for FunctionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FunctionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for FunctionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Unique identifier for a function invocation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct InvocationId(pub Uuid);

impl InvocationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for InvocationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for InvocationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for InvocationId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

// =============================================================================
// Function Metadata
// =============================================================================

/// Complete metadata for a Lambda function
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FunctionMetadata {
    /// Unique identifier
    pub id: FunctionId,
    /// Human-readable name (must be unique)
    pub name: String,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Runtime (currently only "javascript")
    pub runtime: String,
    /// Handler name (e.g., "handler" or "index.handler")
    pub handler: String,
    /// Function code
    pub code: String,
    /// SHA-256 hash of code
    pub code_hash: String,
    /// Environment variables
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Memory limit in MB
    pub memory_mb: u64,
    /// When the function was created (Unix timestamp ms)
    pub created_at: u64,
    /// When the function was last updated (Unix timestamp ms)
    pub updated_at: u64,
}

/// Immutable published version of a function
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FunctionVersion {
    /// Function ID
    pub function_id: FunctionId,
    /// Version number (starts at 1, increments on each publish)
    pub version: u32,
    /// Snapshot of code at publish time
    pub code: String,
    /// SHA-256 hash of code
    pub code_hash: String,
    /// Handler name at publish time
    pub handler: String,
    /// Environment variables at publish time
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Timeout at publish time
    pub timeout_ms: u64,
    /// Memory limit at publish time
    pub memory_mb: u64,
    /// When this version was published (Unix timestamp ms)
    pub published_at: u64,
}

/// Named alias pointing to a specific version
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Alias {
    /// Function ID
    pub function_id: FunctionId,
    /// Alias name (e.g., "dev", "prod", "staging")
    pub alias: String,
    /// Version this alias points to
    pub version: u32,
    /// When the alias was created (Unix timestamp ms)
    pub created_at: u64,
    /// When the alias was last updated (Unix timestamp ms)
    pub updated_at: u64,
}

// =============================================================================
// Requests
// =============================================================================

/// Request to create a new function
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateFunctionRequest {
    /// Function name (must be unique)
    pub name: String,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Runtime (defaults to "javascript")
    #[serde(default = "default_runtime")]
    pub runtime: String,
    /// Handler name (defaults to "handler")
    #[serde(default = "default_handler")]
    pub handler: String,
    /// Function code
    pub code: String,
    /// Environment variables
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Timeout in milliseconds (defaults to 30000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Memory limit in MB (defaults to 128)
    #[serde(default = "default_memory")]
    pub memory_mb: u64,
}

fn default_runtime() -> String {
    "javascript".to_string()
}

fn default_handler() -> String {
    "handler".to_string()
}

fn default_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_memory() -> u64 {
    128 // 128 MB
}

/// Request to update an existing function
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateFunctionRequest {
    /// New description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// New handler name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    /// New code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// New environment variables (replaces existing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<HashMap<String, String>>,
    /// New timeout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// New memory limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
}

// =============================================================================
// Function Reference
// =============================================================================

/// Reference to a function (by ID, name, or name with alias)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FunctionRef {
    /// By UUID
    Id(FunctionId),
    /// By name (uses $LATEST) or name:alias (e.g., "my-func:prod")
    Name(String),
}

impl FunctionRef {
    /// Parse a function reference from a string
    /// Supports:
    /// - UUID: "550e8400-e29b-41d4-a716-446655440000"
    /// - Name: "my-function"
    /// - Name with alias: "my-function:prod"
    pub fn parse(s: &str) -> Self {
        if let Ok(uuid) = Uuid::parse_str(s) {
            FunctionRef::Id(FunctionId(uuid))
        } else {
            FunctionRef::Name(s.to_string())
        }
    }

    /// Get the function name (if this is a name reference)
    pub fn name(&self) -> Option<&str> {
        match self {
            FunctionRef::Name(name) => Some(name.split(':').next().unwrap()),
            FunctionRef::Id(_) => None,
        }
    }

    /// Get the alias (if this is a name:alias reference)
    pub fn alias(&self) -> Option<&str> {
        match self {
            FunctionRef::Name(name) => {
                let parts: Vec<&str> = name.split(':').collect();
                if parts.len() > 1 {
                    Some(parts[1])
                } else {
                    None
                }
            }
            FunctionRef::Id(_) => None,
        }
    }
}

/// Reference to a specific version
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VersionRef {
    /// Latest (unpublished) version
    Latest,
    /// Specific version number
    Number(u32),
    /// Named alias
    Alias(String),
}

impl std::fmt::Display for VersionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionRef::Latest => write!(f, "$LATEST"),
            VersionRef::Number(n) => write!(f, "{}", n),
            VersionRef::Alias(a) => write!(f, "{}", a),
        }
    }
}

impl std::str::FromStr for VersionRef {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "$LATEST" || s.is_empty() {
            Ok(VersionRef::Latest)
        } else if let Ok(n) = s.parse::<u32>() {
            Ok(VersionRef::Number(n))
        } else {
            Ok(VersionRef::Alias(s.to_string()))
        }
    }
}

// =============================================================================
// Invocation
// =============================================================================

/// Invocation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum InvocationType {
    /// Synchronous - wait for result
    RequestResponse,
    /// Asynchronous - return immediately
    Event,
    /// Validate without executing
    DryRun,
}

impl std::fmt::Display for InvocationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvocationType::RequestResponse => write!(f, "RequestResponse"),
            InvocationType::Event => write!(f, "Event"),
            InvocationType::DryRun => write!(f, "DryRun"),
        }
    }
}

impl std::str::FromStr for InvocationType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RequestResponse" => Ok(InvocationType::RequestResponse),
            "Event" => Ok(InvocationType::Event),
            "DryRun" => Ok(InvocationType::DryRun),
            _ => Err(format!("Invalid invocation type: {}", s)),
        }
    }
}

/// Invocation request
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvocationRequest {
    /// Function to invoke
    pub function_ref: FunctionRef,
    /// Event payload
    pub event: Value,
    /// Invocation type
    #[serde(default = "default_invocation_type")]
    pub invocation_type: InvocationType,
}

fn default_invocation_type() -> InvocationType {
    InvocationType::RequestResponse
}

/// Invocation response (for sync invocations)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvocationResponse {
    /// Invocation ID
    pub invocation_id: InvocationId,
    /// Function ID
    pub function_id: FunctionId,
    /// Version executed (None for $LATEST)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Whether this was a cold start
    pub cold_start: bool,
    /// Return value (if successful)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Resource metrics
    pub metrics: ResourceMetrics,
}

// =============================================================================
// Events
// =============================================================================

/// Events emitted during Lambda operations
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LambdaEvent {
    /// Function was created
    FunctionCreated {
        metadata: FunctionMetadata,
    },

    /// Function was updated
    FunctionUpdated {
        metadata: FunctionMetadata,
    },

    /// Function was deleted
    FunctionDeleted {
        function_id: FunctionId,
        name: String,
    },

    /// Version was published
    VersionPublished {
        version: FunctionVersion,
    },

    /// Alias was created or updated
    AliasUpdated {
        alias: Alias,
    },

    /// Invocation started
    InvocationStarted {
        invocation_id: InvocationId,
        function_id: FunctionId,
        function_name: String,
        version: Option<u32>,
    },

    /// Invocation completed
    InvocationCompleted {
        invocation_id: InvocationId,
        success: bool,
        duration_ms: u64,
    },

    /// Forward execution events (console, errors, etc.)
    Execution(JsExecEvent),
}

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur during Lambda operations
#[derive(Debug, Error)]
pub enum LambdaError {
    /// Function not found
    #[error("function not found: {0}")]
    FunctionNotFound(String),

    /// Version not found
    #[error("version not found: {function_id} version {version}")]
    VersionNotFound { function_id: FunctionId, version: u32 },

    /// Alias not found
    #[error("alias not found: {function_id}:{alias}")]
    AliasNotFound { function_id: FunctionId, alias: String },

    /// Function name already exists
    #[error("function name already exists: {0}")]
    NameAlreadyExists(String),

    /// Invalid function name
    #[error("invalid function name: {0}")]
    InvalidName(String),

    /// Invalid handler
    #[error("invalid handler: {0}")]
    InvalidHandler(String),

    /// Database error
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Invocation error
    #[error("invocation error: {0}")]
    Invocation(String),

    /// Internal error
    #[error("internal error: {0}")]
    Internal(String),
}

// =============================================================================
// Metrics
// =============================================================================

/// Metrics for a function over a time range
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FunctionMetrics {
    /// Function ID
    pub function_id: FunctionId,
    /// Total invocations
    pub total_invocations: u64,
    /// Successful invocations
    pub successful_invocations: u64,
    /// Failed invocations
    pub failed_invocations: u64,
    /// Cold starts
    pub cold_starts: u64,
    /// Average duration (ms)
    pub avg_duration_ms: f64,
    /// P50 duration (ms)
    pub p50_duration_ms: u64,
    /// P95 duration (ms)
    pub p95_duration_ms: u64,
    /// P99 duration (ms)
    pub p99_duration_ms: u64,
}

/// Record of a single invocation (for metrics)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationRecord {
    pub invocation_id: InvocationId,
    pub function_id: FunctionId,
    pub function_name: String,
    pub version: Option<u32>,
    pub invocation_type: InvocationType,
    pub cold_start: bool,
    pub duration_ms: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub cpu_time_ms: Option<u64>,
    pub success: bool,
    pub error_message: Option<String>,
    pub error_type: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}
