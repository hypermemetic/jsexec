//! Lambda execution context
//!
//! Provides the context object passed to Lambda handlers following AWS Lambda patterns.

use crate::lambda::types::InvocationId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lambda execution context passed to handler functions
///
/// This context provides information about the current invocation, function,
/// and execution environment, following AWS Lambda's context object pattern.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LambdaContext {
    /// Name of the function being executed
    pub function_name: String,

    /// Function version: version number (e.g., "1") or "$LATEST"
    pub function_version: String,

    /// Unique identifier for this invocation
    pub invocation_id: InvocationId,

    /// Request ID (same as invocation_id for AWS Lambda compatibility)
    pub request_id: InvocationId,

    /// Unix timestamp (in milliseconds) when the function will timeout
    pub deadline_ms: u64,

    /// Optional identity information (for IAM/Cognito authenticated calls)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,

    /// Environment variables available to the function
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Identity information for authenticated invocations
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Identity {
    /// User ID from the identity provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// Amazon Resource Name (ARN) for the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_arn: Option<String>,
}

impl LambdaContext {
    /// Create a new LambdaContext
    ///
    /// # Arguments
    ///
    /// * `function_name` - Name of the function being executed
    /// * `version` - Optional version number (None for $LATEST)
    /// * `timeout_ms` - Timeout duration in milliseconds
    /// * `env` - Environment variables for the function
    ///
    /// # Returns
    ///
    /// A new LambdaContext with a unique invocation ID and calculated deadline
    pub fn new(
        function_name: String,
        version: Option<u32>,
        timeout_ms: u64,
        env: HashMap<String, String>,
    ) -> Self {
        let invocation_id = InvocationId::new();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let function_version = version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "$LATEST".to_string());

        Self {
            function_name,
            function_version,
            invocation_id,
            request_id: invocation_id, // Same as invocation_id for compatibility
            deadline_ms: now_ms + timeout_ms,
            identity: None,
            env,
        }
    }

    /// Calculate remaining time until the function timeout
    ///
    /// # Returns
    ///
    /// Number of milliseconds remaining until deadline.
    /// Returns 0 if the deadline has already passed.
    pub fn remaining_time_ms(&self) -> u64 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.deadline_ms.saturating_sub(now_ms)
    }

    /// Set the identity information for this context
    pub fn with_identity(mut self, identity: Identity) -> Self {
        self.identity = Some(identity);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let mut env = HashMap::new();
        env.insert("NODE_ENV".to_string(), "production".to_string());

        let context = LambdaContext::new(
            "my-function".to_string(),
            Some(1),
            30000, // 30 seconds
            env.clone(),
        );

        assert_eq!(context.function_name, "my-function");
        assert_eq!(context.function_version, "1");
        assert_eq!(context.invocation_id, context.request_id);
        assert_eq!(context.env, env);
        assert!(context.identity.is_none());
    }

    #[test]
    fn test_context_latest_version() {
        let context = LambdaContext::new(
            "my-function".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        assert_eq!(context.function_version, "$LATEST");
    }

    #[test]
    fn test_remaining_time_ms() {
        let context = LambdaContext::new(
            "my-function".to_string(),
            None,
            30000,
            HashMap::new(),
        );

        let remaining = context.remaining_time_ms();
        // Should be close to 30000ms, allowing some overhead
        assert!(remaining <= 30000);
        assert!(remaining > 29000); // Allow 1 second overhead
    }

    #[test]
    fn test_with_identity() {
        let identity = Identity {
            user_id: Some("user-123".to_string()),
            user_arn: Some("arn:aws:iam::123456789:user/test".to_string()),
        };

        let context = LambdaContext::new(
            "my-function".to_string(),
            None,
            30000,
            HashMap::new(),
        )
        .with_identity(identity);

        assert!(context.identity.is_some());
        assert_eq!(
            context.identity.as_ref().unwrap().user_id,
            Some("user-123".to_string())
        );
    }

    #[test]
    fn test_context_serialization() {
        let context = LambdaContext::new(
            "my-function".to_string(),
            Some(2),
            30000,
            HashMap::from([("KEY".to_string(), "value".to_string())]),
        );

        let json = serde_json::to_string(&context).unwrap();
        assert!(json.contains("my-function"));
        assert!(json.contains("\"function_version\":\"2\""));
        assert!(json.contains("KEY"));

        let deserialized: LambdaContext = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.function_name, context.function_name);
        assert_eq!(deserialized.function_version, context.function_version);
    }
}
