//! Function invocation orchestration
//!
//! This module provides the core orchestration layer that ties together function
//! registry, handler runtime, and metrics collection to execute Lambda functions.
//!
//! The invoker handles:
//! - Function reference resolution (ID, name, or name:alias)
//! - Version and alias resolution
//! - Invocation type handling (RequestResponse, Event, DryRun)
//! - Handler execution with proper context
//! - Metrics collection
//! - Error handling and event stream management

use crate::lambda::{
    context::LambdaContext,
    metrics::MetricsCollector,
    registry::FunctionRegistry,
    runtime::HandlerRuntime,
    types::{
        FunctionId, FunctionMetadata, FunctionRef, InvocationId, InvocationRecord,
        InvocationType, InvocationResponse, LambdaError,
    },
};
use crate::types::{JsExecEvent, ResourceMetrics};
use serde_json::Value;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Mode for function invocation
///
/// This is an alias for InvocationType for backward compatibility and
/// clarity in the public API.
pub use crate::lambda::types::InvocationType as InvocationMode;

/// Orchestrates function invocations
///
/// The FunctionInvoker is the main entry point for executing Lambda functions.
/// It coordinates between the function registry (for code/metadata lookup),
/// the handler runtime (for actual execution), and the metrics collector
/// (for observability).
///
/// # Architecture
///
/// ```text
/// ┌─────────────────┐
/// │ FunctionInvoker │
/// └────────┬────────┘
///          │
///     ┌────┴──────────────────┬──────────────────┐
///     ▼                       ▼                  ▼
/// ┌────────────┐      ┌──────────────┐  ┌──────────────┐
/// │  Registry  │      │   Runtime    │  │   Metrics    │
/// │  (lookup)  │      │  (execute)   │  │  (observe)   │
/// └────────────┘      └──────────────┘  └──────────────┘
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use plexus_jsexec::lambda::{FunctionInvoker, FunctionRef, InvocationType};
///
/// let invoker = FunctionInvoker::new(
///     Arc::new(registry),
///     Arc::new(runtime),
///     Some(Arc::new(metrics)),
/// );
///
/// let response = invoker.invoke(
///     FunctionRef::parse("my-function:prod"),
///     serde_json::json!({"key": "value"}),
///     InvocationType::RequestResponse,
/// ).await?;
///
/// println!("Result: {:?}", response.result);
/// ```
pub struct FunctionInvoker {
    /// Function registry for metadata and code lookup
    registry: Arc<FunctionRegistry>,
    /// Handler runtime for executing functions
    runtime: Arc<HandlerRuntime>,
    /// Optional metrics collector for observability
    metrics: Option<Arc<MetricsCollector>>,
}

impl FunctionInvoker {
    /// Create a new function invoker
    ///
    /// # Arguments
    ///
    /// * `registry` - Function registry for metadata and code lookup
    /// * `runtime` - Handler runtime for executing functions
    /// * `metrics` - Optional metrics collector for observability
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let invoker = FunctionInvoker::new(
    ///     Arc::new(registry),
    ///     Arc::new(runtime),
    ///     Some(Arc::new(metrics)),
    /// );
    /// ```
    pub fn new(
        registry: Arc<FunctionRegistry>,
        runtime: Arc<HandlerRuntime>,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> Self {
        Self {
            registry,
            runtime,
            metrics,
        }
    }

    /// Invoke a function by reference
    ///
    /// This is the main invocation method that handles all function reference types:
    /// - UUID: Direct lookup by ID
    /// - Name: Lookup by name, uses $LATEST
    /// - Name:Alias: Lookup by name, resolve alias to version
    ///
    /// # Arguments
    ///
    /// * `function_ref` - Reference to the function (ID, name, or name:alias)
    /// * `event` - Event payload to pass to the function
    /// * `invocation_type` - How to invoke the function
    ///
    /// # Returns
    ///
    /// The invocation response containing result, metrics, and metadata
    ///
    /// # Errors
    ///
    /// * `FunctionNotFound` - Function doesn't exist
    /// * `AliasNotFound` - Specified alias doesn't exist
    /// * `VersionNotFound` - Specified version doesn't exist
    /// * `Invocation` - Error during function execution
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Invoke by name (uses $LATEST)
    /// let response = invoker.invoke(
    ///     FunctionRef::parse("my-function"),
    ///     serde_json::json!({"user_id": 123}),
    ///     InvocationType::RequestResponse,
    /// ).await?;
    ///
    /// // Invoke by name with alias
    /// let response = invoker.invoke(
    ///     FunctionRef::parse("my-function:prod"),
    ///     serde_json::json!({"user_id": 123}),
    ///     InvocationType::RequestResponse,
    /// ).await?;
    /// ```
    pub async fn invoke(
        &self,
        function_ref: FunctionRef,
        event: Value,
        invocation_type: InvocationType,
    ) -> Result<InvocationResponse, LambdaError> {
        // Step 1: Resolve the function reference to get function_id and version
        let (function_id, version) = self.resolve_function_ref(&function_ref).await?;

        // Step 2: Invoke the resolved function and version
        self.invoke_version(&function_id, version, event, invocation_type)
            .await
    }

    /// Invoke a specific version of a function
    ///
    /// This method bypasses function reference resolution and directly invokes
    /// a specific version of a function.
    ///
    /// # Arguments
    ///
    /// * `function_id` - The function ID to invoke
    /// * `version` - The version to invoke (None for $LATEST)
    /// * `event` - Event payload to pass to the function
    /// * `invocation_type` - How to invoke the function
    ///
    /// # Returns
    ///
    /// The invocation response containing result, metrics, and metadata
    ///
    /// # Errors
    ///
    /// * `FunctionNotFound` - Function doesn't exist
    /// * `VersionNotFound` - Specified version doesn't exist
    /// * `Invocation` - Error during function execution
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Invoke $LATEST
    /// let response = invoker.invoke_version(
    ///     &function_id,
    ///     None,
    ///     serde_json::json!({"key": "value"}),
    ///     InvocationType::RequestResponse,
    /// ).await?;
    ///
    /// // Invoke version 5
    /// let response = invoker.invoke_version(
    ///     &function_id,
    ///     Some(5),
    ///     serde_json::json!({"key": "value"}),
    ///     InvocationType::RequestResponse,
    /// ).await?;
    /// ```
    pub async fn invoke_version(
        &self,
        function_id: &FunctionId,
        version: Option<u32>,
        event: Value,
        invocation_type: InvocationType,
    ) -> Result<InvocationResponse, LambdaError> {
        // Step 1: Load function code and metadata
        let (code, metadata) = self.load_function(function_id, version).await?;

        // Step 2: Handle invocation type
        match invocation_type {
            InvocationType::DryRun => {
                // For dry run, just validate and return without executing
                self.handle_dry_run(function_id, version, metadata).await
            }
            InvocationType::Event => {
                // For async invocation, spawn task and return immediately
                self.handle_event_invocation(function_id, version, code, metadata, event)
                    .await
            }
            InvocationType::RequestResponse => {
                // For sync invocation, execute and wait for result
                self.handle_request_response(function_id, version, code, metadata, event)
                    .await
            }
        }
    }

    /// Resolve a function reference to (function_id, version)
    ///
    /// # Arguments
    ///
    /// * `function_ref` - The function reference to resolve
    ///
    /// # Returns
    ///
    /// Tuple of (function_id, version) where version is None for $LATEST
    ///
    /// # Errors
    ///
    /// * `FunctionNotFound` - Function doesn't exist
    /// * `AliasNotFound` - Specified alias doesn't exist
    async fn resolve_function_ref(
        &self,
        function_ref: &FunctionRef,
    ) -> Result<(FunctionId, Option<u32>), LambdaError> {
        match function_ref {
            // Direct ID lookup - use $LATEST
            FunctionRef::Id(id) => {
                // Verify the function exists
                self.registry
                    .get_function(id)
                    .await?;

                Ok((*id, None))
            }

            // Name or name:alias lookup
            FunctionRef::Name(name) => {
                // Check if this is a name:alias reference
                if let Some(alias) = function_ref.alias() {
                    // Resolve name:alias
                    let function_name = function_ref.name().unwrap();
                    let metadata = self
                        .registry
                        .get_function_by_name(function_name)
                        .await?;

                    // Resolve the alias to a version number
                    let alias_record = self
                        .registry
                        .get_alias(&metadata.id, alias)
                        .await?;

                    Ok((metadata.id, Some(alias_record.version)))
                } else {
                    // Just a name, use $LATEST
                    let metadata = self
                        .registry
                        .get_function_by_name(name)
                        .await?;

                    Ok((metadata.id, None))
                }
            }
        }
    }

    /// Load function code and metadata
    ///
    /// # Arguments
    ///
    /// * `function_id` - The function to load
    /// * `version` - The version to load (None for $LATEST)
    ///
    /// # Returns
    ///
    /// Tuple of (code, metadata)
    ///
    /// # Errors
    ///
    /// * `FunctionNotFound` - Function doesn't exist
    /// * `VersionNotFound` - Specified version doesn't exist
    async fn load_function(
        &self,
        function_id: &FunctionId,
        version: Option<u32>,
    ) -> Result<(String, FunctionMetadata), LambdaError> {
        if let Some(version_num) = version {
            // Load a specific published version
            let version_data = self
                .registry
                .get_version(function_id, version_num)
                .await?;

            // Get base metadata for the function name
            let metadata = self
                .registry
                .get_function(function_id)
                .await?;

            // Build metadata from version snapshot
            let function_metadata = FunctionMetadata {
                id: version_data.function_id,
                name: metadata.name, // Use current name, not snapshot
                description: metadata.description,
                runtime: metadata.runtime,
                handler: version_data.handler,
                code: version_data.code.clone(),
                code_hash: version_data.code_hash,
                environment: version_data.environment,
                timeout_ms: version_data.timeout_ms,
                memory_mb: version_data.memory_mb,
                created_at: metadata.created_at,
                updated_at: version_data.published_at,
            };

            Ok((version_data.code, function_metadata))
        } else {
            // Load $LATEST
            let metadata = self
                .registry
                .get_function(function_id)
                .await?;

            Ok((metadata.code.clone(), metadata))
        }
    }

    /// Handle DryRun invocation type
    ///
    /// Validates that the function exists and code is valid without executing.
    async fn handle_dry_run(
        &self,
        function_id: &FunctionId,
        version: Option<u32>,
        _metadata: FunctionMetadata,
    ) -> Result<InvocationResponse, LambdaError> {
        // For dry run, we just return success with empty result
        // The fact that we got here means the function exists and code loaded
        let invocation_id = InvocationId::new();

        Ok(InvocationResponse {
            invocation_id,
            function_id: *function_id,
            version,
            cold_start: false, // Dry run doesn't actually start anything
            result: Some(Value::Null),
            error: None,
            metrics: ResourceMetrics::default(),
        })
    }

    /// Handle Event (async) invocation type
    ///
    /// Spawns a background task to execute the function and returns immediately.
    async fn handle_event_invocation(
        &self,
        function_id: &FunctionId,
        version: Option<u32>,
        code: String,
        metadata: FunctionMetadata,
        event: Value,
    ) -> Result<InvocationResponse, LambdaError> {
        let invocation_id = InvocationId::new();
        let function_id = *function_id;

        // Clone necessary data for the background task
        let runtime = Arc::clone(&self.runtime);
        let metrics_collector = self.metrics.clone();

        // Spawn background task for async execution
        tokio::spawn(async move {
            // Create Lambda context
            let context = LambdaContext::new(
                metadata.name.clone(),
                version,
                metadata.timeout_ms,
                metadata.environment.clone(),
            );

            // Execute the handler (fire and forget)
            let event_stream = runtime
                .invoke_handler(
                    code,
                    metadata.handler,
                    event,
                    context,
                );

            // Collect events in background
            let mut final_metrics = ResourceMetrics::default();
            let mut success = true;
            let started_at = current_timestamp_ms();
            let mut completed_at = None;

            let mut event_stream = std::pin::pin!(event_stream);
            while let Some(js_event) = futures::StreamExt::next(&mut event_stream).await {
                match js_event {
                    JsExecEvent::ExecutionCompleted { metrics, .. } => {
                        final_metrics = metrics;
                        completed_at = Some(current_timestamp_ms());
                    }
                    JsExecEvent::Error { .. } => {
                        success = false;
                    }
                    _ => {
                        // Ignore other events in background execution
                    }
                }
            }

            // Record metrics if collector is available
            if let Some(collector) = metrics_collector {
                let record = InvocationRecord {
                    invocation_id,
                    function_id,
                    function_name: metadata.name,
                    version,
                    invocation_type: InvocationType::Event,
                    cold_start: false, // TODO: Track cold starts properly
                    duration_ms: completed_at.map(|c| c - started_at),
                    memory_used_bytes: Some(final_metrics.memory_used_bytes),
                    cpu_time_ms: Some(final_metrics.cpu_time_ms),
                    success,
                    error_message: None,
                    error_type: None,
                    started_at,
                    completed_at,
                };

                // Fire and forget - don't await
                let _ = collector.record_invocation(record).await;
            }
        });

        // Return immediately with invocation ID
        Ok(InvocationResponse {
            invocation_id,
            function_id,
            version,
            cold_start: false, // TODO: Track cold starts properly
            result: None,      // Async invocations don't return results
            error: None,
            metrics: ResourceMetrics::default(),
        })
    }

    /// Handle RequestResponse (sync) invocation type
    ///
    /// Executes the function synchronously and waits for the result.
    async fn handle_request_response(
        &self,
        function_id: &FunctionId,
        version: Option<u32>,
        code: String,
        metadata: FunctionMetadata,
        event: Value,
    ) -> Result<InvocationResponse, LambdaError> {
        let invocation_id = InvocationId::new();
        let started_at = current_timestamp_ms();

        // Create Lambda context
        let context = LambdaContext::new(
            metadata.name.clone(),
            version,
            metadata.timeout_ms,
            metadata.environment.clone(),
        );

        // Execute the handler
        let event_stream = self
            .runtime
            .invoke_handler(
                code,
                metadata.handler,
                event,
                context,
            );

        // Collect events from the stream
        let mut result: Option<Value> = None;
        let mut error: Option<String> = None;
        let mut final_metrics = ResourceMetrics::default();
        let mut success = true;

        let mut event_stream = std::pin::pin!(event_stream);
        while let Some(js_event) = futures::StreamExt::next(&mut event_stream).await {
            match js_event {
                JsExecEvent::Returned { value } => {
                    result = Some(value);
                }
                JsExecEvent::Error { message, .. } => {
                    error = Some(message.clone());
                    success = false;
                }
                JsExecEvent::ExecutionCompleted { metrics, .. } => {
                    final_metrics = metrics;
                }
                JsExecEvent::Console { .. } => {
                    // TODO: Stream console output to logs
                    // For now, we just ignore console output
                }
                _ => {
                    // Ignore other events
                }
            }
        }

        let completed_at = current_timestamp_ms();
        let duration_ms = completed_at - started_at;

        // Record metrics if collector is available
        if let Some(collector) = &self.metrics {
            let record = InvocationRecord {
                invocation_id,
                function_id: *function_id,
                function_name: metadata.name.clone(),
                version,
                invocation_type: InvocationType::RequestResponse,
                cold_start: false, // TODO: Track cold starts properly
                duration_ms: Some(duration_ms),
                memory_used_bytes: Some(final_metrics.memory_used_bytes),
                cpu_time_ms: Some(final_metrics.cpu_time_ms),
                success,
                error_message: error.clone(),
                error_type: if error.is_some() {
                    Some("ExecutionError".to_string())
                } else {
                    None
                },
                started_at,
                completed_at: Some(completed_at),
            };

            collector.record_invocation(record).await?;
        }

        // Build response
        Ok(InvocationResponse {
            invocation_id,
            function_id: *function_id,
            version,
            cold_start: false, // TODO: Track cold starts properly
            result,
            error,
            metrics: final_metrics,
        })
    }
}

/// Get current timestamp in milliseconds since UNIX epoch
fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lambda::types::{CreateFunctionRequest, VersionRef};

    // TODO: These tests will be implemented once all dependencies are complete
    // They serve as documentation of expected behavior

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_by_id() {
        // Setup: Create function, get ID
        // Act: Invoke by FunctionRef::Id
        // Assert: Function executes, returns result
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_by_name() {
        // Setup: Create function with name
        // Act: Invoke by FunctionRef::Name (uses $LATEST)
        // Assert: Function executes, returns result
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_by_name_with_alias() {
        // Setup: Create function, publish version, create alias
        // Act: Invoke by "name:alias"
        // Assert: Executes correct version, returns result
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_version_latest() {
        // Setup: Create function, update code
        // Act: Invoke with version=None
        // Assert: Executes latest code
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_version_specific() {
        // Setup: Create function, publish version 1, update code
        // Act: Invoke with version=Some(1)
        // Assert: Executes version 1 code (not updated code)
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_dry_run() {
        // Setup: Create function
        // Act: Invoke with InvocationType::DryRun
        // Assert: Returns success, doesn't execute handler
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_event_async() {
        // Setup: Create function
        // Act: Invoke with InvocationType::Event
        // Assert: Returns immediately with invocation_id, no result
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_request_response() {
        // Setup: Create function that returns value
        // Act: Invoke with InvocationType::RequestResponse
        // Assert: Returns result from handler
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_with_error() {
        // Setup: Create function that throws error
        // Act: Invoke with InvocationType::RequestResponse
        // Assert: Response contains error, no result
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_function_not_found() {
        // Setup: None
        // Act: Try to invoke non-existent function
        // Assert: Returns FunctionNotFound error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_version_not_found() {
        // Setup: Create function, publish version 1
        // Act: Try to invoke version 999
        // Assert: Returns VersionNotFound error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_invoke_alias_not_found() {
        // Setup: Create function
        // Act: Try to invoke "name:nonexistent"
        // Assert: Returns AliasNotFound error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_metrics_collection() {
        // Setup: Create invoker with metrics collector
        // Act: Invoke function
        // Assert: Metrics are recorded in collector
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_context_creation() {
        // Setup: Create function with env vars and timeout
        // Act: Invoke function that accesses context
        // Assert: Context has correct values
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_timeout_handling() {
        // Setup: Create function with short timeout that runs long
        // Act: Invoke function
        // Assert: Returns timeout error with appropriate metrics
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_console_output_collection() {
        // Setup: Create function that logs to console
        // Act: Invoke function
        // Assert: Console events are captured in stream
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry and HandlerRuntime implementation"]
    async fn test_concurrent_invocations() {
        // Setup: Create function
        // Act: Invoke same function multiple times concurrently
        // Assert: All invocations succeed with unique IDs
    }
}
