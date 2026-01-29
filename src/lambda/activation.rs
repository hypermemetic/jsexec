//! LambdaExec activation - AWS Lambda-style function execution
//!
//! This module provides the main LambdaExec activation that integrates with
//! the Plexus hub system using hub_macro.

use crate::lambda::{
    types::{
        CreateFunctionRequest, FunctionId, FunctionMetadata, FunctionRef, FunctionVersion,
        InvocationType, LambdaError, LambdaEvent, UpdateFunctionRequest, VersionRef,
    },
    LambdaSystem,
};
use async_stream::stream;
use futures::Stream;
use std::collections::HashMap;
use std::sync::Arc;

// Re-export hub_core types needed by hub_macro generated code
#[allow(unused_imports)]
use hub_core::plexus::{
    Activation, ChildSummary, MethodSchema, PlexusError, PlexusStream, PluginSchema, SchemaResult,
    wrap_stream,
};
#[allow(unused_imports)]
use hub_core::serde_helpers;

// =============================================================================
// LambdaExec Activation
// =============================================================================

/// LambdaExec activation - execute AWS Lambda-style functions
#[derive(Clone)]
pub struct LambdaExec {
    system: Arc<LambdaSystem>,
}

impl LambdaExec {
    /// Create a new LambdaExec instance with the given system
    pub fn new(system: Arc<LambdaSystem>) -> Self {
        Self { system }
    }
}

/// Hub-macro generates all the boilerplate for this impl block
#[hub_macro::hub_methods(
    namespace = "lambda",
    version = "1.0.0",
    description = "AWS Lambda-style function execution",
    crate_path = "hub_core"
)]
impl LambdaExec {
    /// Create a new Lambda function
    #[hub_macro::hub_method(
        description = "Create a new Lambda function",
        params(
            name = "Function name (must be unique)",
            code = "Function code (JavaScript)",
            handler = "Handler name (default: 'handler')",
            runtime = "Runtime (default: 'javascript')",
            description = "Optional description",
            timeout_ms = "Timeout in milliseconds (default: 30000)",
            environment = "Environment variables"
        )
    )]
    async fn create_function(
        &self,
        name: String,
        code: String,
        #[hub_macro::arg(default = "handler")] handler: String,
        #[hub_macro::arg(default = "javascript")] runtime: String,
        description: Option<String>,
        timeout_ms: Option<u64>,
        environment: Option<HashMap<String, String>>,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Build the create request
            let request = CreateFunctionRequest {
                name: name.clone(),
                description,
                runtime,
                handler,
                code,
                environment: environment.unwrap_or_default(),
                timeout_ms: timeout_ms.unwrap_or(30000),
                memory_mb: 128, // Default memory
            };

            // Create the function via registry
            match system.registry.create_function(request).await {
                Ok(metadata) => {
                    yield LambdaEvent::FunctionCreated { metadata };
                }
                Err(e) => {
                    yield error_to_event(e);
                }
            }
        }
    }

    /// Invoke a Lambda function
    #[hub_macro::hub_method(
        description = "Invoke a Lambda function",
        params(
            function_name = "Function name or name:alias (e.g., 'my-func' or 'my-func:prod')",
            event = "Event payload (JSON)",
            invocation_type = "Invocation type: 'RequestResponse', 'Event', or 'DryRun' (default: 'RequestResponse')"
        )
    )]
    async fn invoke(
        &self,
        function_name: String,
        event: serde_json::Value,
        #[hub_macro::arg(default = "RequestResponse")] invocation_type: String,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Parse invocation type
            let invocation_type = match invocation_type.parse::<InvocationType>() {
                Ok(t) => t,
                Err(e) => {
                    yield error_to_event(LambdaError::Internal(e));
                    return;
                }
            };

            // Parse function reference (supports "name" or "name:alias")
            let function_ref = FunctionRef::parse(&function_name);

            // Resolve the function to get metadata
            let (function_id, metadata, version) = match resolve_function_ref(&system, &function_ref).await {
                Ok(result) => result,
                Err(e) => {
                    yield error_to_event(e);
                    return;
                }
            };

            // Generate invocation ID
            let invocation_id = crate::lambda::types::InvocationId::new();

            // Yield invocation started event
            yield LambdaEvent::InvocationStarted {
                invocation_id,
                function_id,
                function_name: metadata.name.clone(),
                version,
            };

            // Record start time for metrics
            let start_time = std::time::Instant::now();

            // Get the code to execute (from version or $LATEST)
            let code = if let Some(v) = version {
                // Get code from specific version
                match system.registry.get_version(&function_id, v).await {
                    Ok(Some(version_metadata)) => version_metadata.code,
                    Ok(None) => {
                        yield error_to_event(LambdaError::VersionNotFound {
                            function_id,
                            version: v,
                        });
                        return;
                    }
                    Err(e) => {
                        yield error_to_event(e);
                        return;
                    }
                }
            } else {
                // Use $LATEST code
                metadata.code.clone()
            };

            // Create Lambda context
            let context = crate::lambda::LambdaContext::new(
                metadata.name.clone(),
                version,
                metadata.timeout_ms,
                metadata.environment.clone(),
            );

            // Stream execution events from the runtime
            let execution_stream = system.runtime.invoke_handler(
                code,
                metadata.handler.clone(),
                event,
                context,
            );

            use futures::StreamExt;
            let mut execution_stream = std::pin::pin!(execution_stream);

            // Track success/failure for completion event
            let mut success = true;

            while let Some(exec_event) = execution_stream.next().await {
                // Check if this is an error event
                if matches!(exec_event, crate::types::JsExecEvent::Error { .. }) {
                    success = false;
                }

                // Forward execution event
                yield LambdaEvent::Execution(exec_event);
            }

            // Yield invocation completed event
            let duration_ms = start_time.elapsed().as_millis() as u64;
            yield LambdaEvent::InvocationCompleted {
                invocation_id,
                success,
                duration_ms,
            };
        }
    }

    /// Update function code
    #[hub_macro::hub_method(
        description = "Update function code",
        params(
            function_name = "Function name",
            code = "New function code (JavaScript)"
        )
    )]
    async fn update_function_code(
        &self,
        function_name: String,
        code: String,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Get function by name
            let metadata = match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    yield error_to_event(LambdaError::FunctionNotFound(function_name));
                    return;
                }
                Err(e) => {
                    yield error_to_event(e);
                    return;
                }
            };

            // Update the function
            let request = UpdateFunctionRequest {
                code: Some(code),
                ..Default::default()
            };

            match system.registry.update_function(&metadata.id, request).await {
                Ok(updated_metadata) => {
                    yield LambdaEvent::FunctionUpdated { metadata: updated_metadata };
                }
                Err(e) => {
                    yield error_to_event(e);
                }
            }
        }
    }

    /// Publish a new version
    #[hub_macro::hub_method(
        description = "Publish a new immutable version",
        params(function_name = "Function name")
    )]
    async fn publish_version(
        &self,
        function_name: String,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Get function by name
            let metadata = match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    yield error_to_event(LambdaError::FunctionNotFound(function_name));
                    return;
                }
                Err(e) => {
                    yield error_to_event(e);
                    return;
                }
            };

            // Publish version
            match system.versioning.publish_version(&metadata.id).await {
                Ok(version) => {
                    yield LambdaEvent::VersionPublished { version };
                }
                Err(e) => {
                    yield error_to_event(e);
                }
            }
        }
    }

    /// Create or update an alias
    #[hub_macro::hub_method(
        description = "Create or update a function alias",
        params(
            function_name = "Function name",
            alias = "Alias name (e.g., 'dev', 'prod', 'staging')",
            version = "Version number (e.g., '1', '2') or '$LATEST'"
        )
    )]
    async fn create_alias(
        &self,
        function_name: String,
        alias: String,
        version: String,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Get function by name
            let metadata = match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    yield error_to_event(LambdaError::FunctionNotFound(function_name));
                    return;
                }
                Err(e) => {
                    yield error_to_event(e);
                    return;
                }
            };

            // Parse version reference
            let version_ref = match version.parse::<VersionRef>() {
                Ok(v) => v,
                Err(_) => {
                    yield error_to_event(LambdaError::Internal(format!("Invalid version: {}", version)));
                    return;
                }
            };

            // Create or update alias
            match system.versioning.upsert_alias(&metadata.id, alias, &version_ref).await {
                Ok(alias_record) => {
                    yield LambdaEvent::AliasUpdated { alias: alias_record };
                }
                Err(e) => {
                    yield error_to_event(e);
                }
            }
        }
    }

    /// List all functions
    #[hub_macro::hub_method(description = "List all Lambda functions")]
    async fn list_functions(&self) -> impl Stream<Item = FunctionMetadata> + Send + 'static {
        let system = self.system.clone();

        stream! {
            match system.registry.list_functions().await {
                Ok(functions) => {
                    for metadata in functions {
                        yield metadata;
                    }
                }
                Err(_e) => {
                    // For list operations, we just return empty stream on error
                    // The error is logged internally
                }
            }
        }
    }

    /// Get function details
    #[hub_macro::hub_method(
        description = "Get function details",
        params(function_name = "Function name")
    )]
    async fn get_function(
        &self,
        function_name: String,
    ) -> impl Stream<Item = FunctionMetadata> + Send + 'static {
        let system = self.system.clone();

        stream! {
            match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(metadata)) => {
                    yield metadata;
                }
                Ok(None) => {
                    // Function not found - return empty stream
                }
                Err(_e) => {
                    // Error - return empty stream
                }
            }
        }
    }

    /// Delete a function
    #[hub_macro::hub_method(
        description = "Delete a Lambda function",
        params(function_name = "Function name")
    )]
    async fn delete_function(
        &self,
        function_name: String,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Get function by name
            let metadata = match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    yield error_to_event(LambdaError::FunctionNotFound(function_name.clone()));
                    return;
                }
                Err(e) => {
                    yield error_to_event(e);
                    return;
                }
            };

            // Delete the function
            match system.registry.delete_function(&metadata.id).await {
                Ok(_) => {
                    yield LambdaEvent::FunctionDeleted {
                        function_id: metadata.id,
                        name: function_name,
                    };
                }
                Err(e) => {
                    yield error_to_event(e);
                }
            }
        }
    }

    /// List versions for a function
    #[hub_macro::hub_method(
        description = "List all versions of a function",
        params(function_name = "Function name")
    )]
    async fn list_versions(
        &self,
        function_name: String,
    ) -> impl Stream<Item = FunctionVersion> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Get function by name
            let metadata = match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    return;
                }
                Err(_e) => {
                    return;
                }
            };

            // List versions
            match system.registry.list_versions(&metadata.id).await {
                Ok(versions) => {
                    for version in versions {
                        yield version;
                    }
                }
                Err(_e) => {
                    // Error - return empty stream
                }
            }
        }
    }

    /// Get function metrics
    #[hub_macro::hub_method(
        description = "Get invocation metrics for a function",
        params(
            function_name = "Function name",
            start_time = "Start time (Unix timestamp ms)",
            end_time = "End time (Unix timestamp ms)"
        )
    )]
    async fn get_metrics(
        &self,
        function_name: String,
        start_time: Option<u64>,
        end_time: Option<u64>,
    ) -> impl Stream<Item = LambdaEvent> + Send + 'static {
        let system = self.system.clone();

        stream! {
            // Get function by name
            let metadata = match system.registry.get_function_by_name(&function_name).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    yield error_to_event(LambdaError::FunctionNotFound(function_name));
                    return;
                }
                Err(e) => {
                    yield error_to_event(e);
                    return;
                }
            };

            // Get metrics from collector (if available)
            let metrics_result = if let Some(ref metrics) = system.metrics {
                metrics.get_function_metrics(&metadata.id, start_time, end_time).await
            } else {
                Err(LambdaError::Internal("Metrics collection not enabled".to_string()))
            };

            match metrics_result {
                Ok(metrics) => {
                    // Wrap metrics in a console log event for now
                    // TODO: Define a proper LambdaEvent::Metrics variant
                    let metrics_json = serde_json::to_string_pretty(&metrics)
                        .unwrap_or_else(|_| "{}".to_string());
                    yield LambdaEvent::Execution(crate::types::JsExecEvent::Console {
                        level: "info".to_string(),
                        message: format!("Metrics for {}: {}", function_name, metrics_json),
                    });
                }
                Err(e) => {
                    yield error_to_event(e);
                }
            }
        }
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Convert a LambdaError into a LambdaEvent
fn error_to_event(error: LambdaError) -> LambdaEvent {
    LambdaEvent::Execution(crate::types::JsExecEvent::Error {
        message: error.to_string(),
        name: "LambdaError".to_string(),
        location: None,
        stack: vec![],
    })
}

/// Resolve a function reference to (function_id, metadata, version)
///
/// This handles:
/// - Function name -> get by name, version = None
/// - Function name:alias -> get by name, resolve alias to version
async fn resolve_function_ref(
    system: &LambdaSystem,
    function_ref: &FunctionRef,
) -> Result<(FunctionId, FunctionMetadata, Option<u32>), LambdaError> {
    match function_ref {
        FunctionRef::Id(id) => {
            let metadata = system
                .registry
                .get_function(id)
                .await?
                .ok_or_else(|| LambdaError::FunctionNotFound(id.to_string()))?;
            Ok((*id, metadata, None))
        }
        FunctionRef::Name(name_with_alias) => {
            // Parse name and optional alias
            let parts: Vec<&str> = name_with_alias.split(':').collect();
            let name = parts[0];
            let alias = if parts.len() > 1 {
                Some(parts[1])
            } else {
                None
            };

            // Get function by name
            let metadata = system
                .registry
                .get_function_by_name(name)
                .await?
                .ok_or_else(|| LambdaError::FunctionNotFound(name.to_string()))?;

            // Resolve alias if provided
            let version = if let Some(alias_name) = alias {
                let version_ref = VersionRef::Alias(alias_name.to_string());
                system
                    .versioning
                    .resolve_version(&metadata.id, &version_ref)
                    .await?
            } else {
                None // Use $LATEST
            };

            Ok((metadata.id, metadata, version))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lambda_exec_new() {
        // This test verifies that LambdaExec can be constructed
        // Actual functionality tests will be added once LambdaSystem is implemented
        // For now, we just verify the structure compiles
    }
}
