//! Handler execution runtime for Lambda functions
//!
//! This module provides the runtime layer that wraps user code to execute
//! Lambda-style handlers following the AWS Lambda pattern:
//! `export async function handler(event, context)`

use crate::lambda::context::LambdaContext;
use crate::runner::{execute, RunnerConfig};
use crate::types::{ExecutionContext, ExecutionLimits, JsExecEvent};
use serde_json::Value;
use tokio_stream::Stream;

/// Runtime for executing Lambda handler functions
///
/// The HandlerRuntime wraps user code to extract and invoke handler functions
/// following the AWS Lambda pattern. It generates wrapper JavaScript that:
/// 1. Loads the user module
/// 2. Extracts the handler (supports named exports and default exports)
/// 3. Injects the event and context objects
/// 4. Calls the handler and returns the result
pub struct HandlerRuntime {
    runner_config: RunnerConfig,
}

impl HandlerRuntime {
    /// Create a new HandlerRuntime with the given configuration
    ///
    /// # Arguments
    ///
    /// * `config` - RunnerConfig for the underlying JavaScript execution engine
    pub fn new(config: RunnerConfig) -> Self {
        Self {
            runner_config: config,
        }
    }

    /// Invoke a Lambda handler function
    ///
    /// # Arguments
    ///
    /// * `function_code` - The user's JavaScript code containing the handler
    /// * `handler_name` - Name of the handler function (e.g., "handler" or "index.handler")
    /// * `event` - Event payload to pass to the handler
    /// * `context` - Lambda context object
    ///
    /// # Returns
    ///
    /// A stream of JsExecEvent items representing the execution progress
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use plexus_jsexec::lambda::{HandlerRuntime, LambdaContext};
    /// use plexus_jsexec::RunnerConfig;
    /// use serde_json::json;
    /// use futures_util::StreamExt;
    /// use std::collections::HashMap;
    ///
    /// # async fn example() {
    /// let runtime = HandlerRuntime::new(RunnerConfig::default());
    /// let code = r#"
    ///     export async function handler(event, context) {
    ///         return { statusCode: 200, body: `Hello ${event.name}` };
    ///     }
    /// "#;
    ///
    /// let context = LambdaContext::new(
    ///     "my-function".to_string(),
    ///     None,
    ///     30000,
    ///     HashMap::new(),
    /// );
    ///
    /// let stream = runtime.invoke_handler(
    ///     code.to_string(),
    ///     "handler".to_string(),
    ///     json!({"name": "World"}),
    ///     context,
    /// );
    ///
    /// tokio::pin!(stream);
    /// while let Some(event) = stream.next().await {
    ///     println!("Event: {:?}", event);
    /// }
    /// # }
    /// ```
    pub fn invoke_handler(
        &self,
        function_code: String,
        handler_name: String,
        event: Value,
        context: LambdaContext,
    ) -> impl Stream<Item = JsExecEvent> {
        // Generate the wrapper code that will load and invoke the handler
        let wrapper_code = generate_handler_wrapper(&function_code, &handler_name, &event, &context);

        // Create execution context (empty - handler gets context via parameters)
        let execution_context = ExecutionContext::default();

        // Use timeout from context's remaining time
        let limits = ExecutionLimits {
            wall_time_ms: Some(context.remaining_time_ms()),
            ..Default::default()
        };

        // Execute the wrapped code using the existing runner
        execute(wrapper_code, execution_context, limits, self.runner_config.clone())
    }
}

/// Generate the wrapper JavaScript that loads and invokes the handler
///
/// This function creates a JavaScript module that:
/// 1. Defines the user's function code as a module
/// 2. Extracts the handler (supports both named and default exports)
/// 3. Validates that the handler is a function
/// 4. Injects the event and context as parameters
/// 5. Invokes the handler and returns the result
///
/// # Arguments
///
/// * `user_code` - The user's JavaScript code
/// * `handler_name` - Name of the handler to extract
/// * `event` - Event payload to serialize and inject
/// * `context` - Context object to serialize and inject
///
/// # Returns
///
/// Complete JavaScript code ready for execution
fn generate_handler_wrapper(
    user_code: &str,
    handler_name: &str,
    event: &Value,
    context: &LambdaContext,
) -> String {
    // Serialize event and context to JSON for injection
    let event_json = serde_json::to_string(event)
        .unwrap_or_else(|_| "null".to_string());
    let context_json = serde_json::to_string(context)
        .unwrap_or_else(|_| "{}".to_string());

    // For now, we only support simple handler names like "handler"
    // Future enhancement: support "module.handler" for multi-file modules
    let handler_identifier = handler_name;

    format!(
        r#"
// ===== User Module Definition =====
// The user's code is wrapped in a function to create a module scope
const userModuleFactory = () => {{
    const exports = {{}};
    const module = {{ exports }};

    // User code executes here
    {user_code}

    // Return exports (supports both CommonJS-style and ES6-style exports)
    return module.exports.default || module.exports || exports;
}};

// Execute the module factory to get the exports
const userModule = userModuleFactory();

// ===== Handler Extraction =====
// Extract the handler - support both named export and default export
let handler;

// Try to get the handler by name first
if (typeof userModule === 'object' && userModule !== null) {{
    // Check for named export
    handler = userModule['{handler_identifier}'];

    // If not found and looking for 'handler', try default export
    if (!handler && '{handler_identifier}' === 'handler') {{
        handler = userModule.default;
    }}
}} else if (typeof userModule === 'function') {{
    // The module itself is a function (default export)
    handler = userModule;
}}

// Validate handler exists and is a function
if (typeof handler !== 'function') {{
    throw new Error(
        'Handler "{handler_identifier}" not found or is not a function. ' +
        'Expected: export function {handler_identifier}(event, context) or export default function(event, context)'
    );
}}

// ===== Event and Context Injection =====
const event = {event_json};
const context = {context_json};

// Add getRemainingTimeInMillis method to context (AWS Lambda compatibility)
context.getRemainingTimeInMillis = function() {{
    const now = Date.now();
    return Math.max(0, this.deadline_ms - now);
}};

// ===== Handler Invocation =====
// Invoke the handler with event and context
// The handler may be async or sync, so we wrap in Promise.resolve
const result = await Promise.resolve(handler(event, context));

// Return the result
return result;
"#,
        user_code = user_code,
        handler_identifier = handler_identifier,
        event_json = event_json,
        context_json = context_json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_generate_handler_wrapper_basic() {
        let user_code = r#"
            export function handler(event, context) {
                return { message: "Hello" };
            }
        "#;

        let event = serde_json::json!({"name": "test"});
        let context = LambdaContext::new(
            "test-function".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Verify wrapper contains key components
        assert!(wrapper.contains(user_code));
        assert!(wrapper.contains("const event ="));
        assert!(wrapper.contains("const context ="));
        assert!(wrapper.contains(r#""name":"test""#));
        assert!(wrapper.contains("test-function"));
        assert!(wrapper.contains("await Promise.resolve(handler(event, context))"));
    }

    #[test]
    fn test_generate_handler_wrapper_default_export() {
        let user_code = r#"
            export default async function(event, context) {
                return { message: "Hello" };
            }
        "#;

        let event = serde_json::json!({});
        let context = LambdaContext::new(
            "test-function".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Should support default export when looking for 'handler'
        assert!(wrapper.contains("userModule.default"));
        assert!(wrapper.contains(user_code));
    }

    #[test]
    fn test_generate_handler_wrapper_with_env() {
        let user_code = "export function handler(e, c) { return c.env; }";
        let event = serde_json::json!(null);

        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "secret-123".to_string());
        env.insert("NODE_ENV".to_string(), "production".to_string());

        let context = LambdaContext::new(
            "test-function".to_string(),
            Some(1),
            30000,
            env,
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Verify environment variables are serialized into context
        assert!(wrapper.contains("API_KEY"));
        assert!(wrapper.contains("secret-123"));
        assert!(wrapper.contains("NODE_ENV"));
        assert!(wrapper.contains("production"));
    }

    #[test]
    fn test_generate_handler_wrapper_error_handling() {
        let user_code = "// No handler exported";
        let event = serde_json::json!({});
        let context = LambdaContext::new(
            "test-function".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Verify error message is included
        assert!(wrapper.contains(r#"Handler "handler" not found or is not a function"#));
        assert!(wrapper.contains("export function handler"));
    }

    #[test]
    fn test_generate_handler_wrapper_complex_event() {
        let user_code = "export function handler(event) { return event; }";
        let event = serde_json::json!({
            "httpMethod": "POST",
            "path": "/api/users",
            "body": "{\"name\":\"John\"}",
            "headers": {
                "Content-Type": "application/json"
            }
        });
        let context = LambdaContext::new(
            "api-function".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Verify complex event structure is preserved
        assert!(wrapper.contains("httpMethod"));
        assert!(wrapper.contains("POST"));
        assert!(wrapper.contains("/api/users"));
        assert!(wrapper.contains("Content-Type"));
    }

    #[test]
    fn test_generate_handler_wrapper_version_context() {
        let user_code = "export function handler(e, c) { return c.function_version; }";
        let event = serde_json::json!({});

        let context = LambdaContext::new(
            "versioned-function".to_string(),
            Some(5),
            30000,
            HashMap::new(),
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Verify version is in the context
        assert!(wrapper.contains(r#""function_version":"5""#));
    }

    #[test]
    fn test_generate_handler_wrapper_adds_get_remaining_time() {
        let user_code = "export function handler() {}";
        let event = serde_json::json!({});
        let context = LambdaContext::new(
            "test".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        let wrapper = generate_handler_wrapper(user_code, "handler", &event, &context);

        // Verify getRemainingTimeInMillis is added
        assert!(wrapper.contains("context.getRemainingTimeInMillis = function()"));
        assert!(wrapper.contains("this.deadline_ms"));
    }
}
