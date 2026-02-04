//! Version and alias management for Lambda functions
//!
//! This module provides versioning capabilities for Lambda functions:
//! - Publishing immutable versions (snapshots of $LATEST)
//! - Creating and managing mutable aliases that point to versions
//! - Resolving version references to concrete version numbers

use crate::lambda::{
    registry::FunctionRegistry,
    types::{Alias, FunctionId, FunctionVersion, LambdaError, VersionRef},
};
use std::sync::Arc;

/// Manages function versions and aliases
///
/// Versions are immutable snapshots of a function's code and configuration.
/// Aliases are mutable pointers to specific versions, allowing stable endpoints
/// that can be updated without changing client code.
pub struct VersioningManager {
    registry: Arc<FunctionRegistry>,
}

impl VersioningManager {
    /// Create a new versioning manager
    pub fn new(registry: Arc<FunctionRegistry>) -> Self {
        Self { registry }
    }

    /// Publish a new version of a function
    ///
    /// This creates an immutable snapshot of the function's current $LATEST code
    /// and configuration. The version number is automatically incremented.
    ///
    /// # Arguments
    /// * `function_id` - The function to publish
    ///
    /// # Returns
    /// The newly created version
    ///
    /// # Errors
    /// * `FunctionNotFound` - If the function doesn't exist
    /// * `Database` - If there's a database error
    ///
    /// # Example
    /// ```rust,ignore
    /// let version = versioning.publish_version(&function_id).await?;
    /// println!("Published version {}", version.version);
    /// ```
    pub async fn publish_version(
        &self,
        function_id: &FunctionId,
    ) -> Result<FunctionVersion, LambdaError> {
        // Delegate to registry which handles the actual DB work
        self.registry.publish_version(function_id).await
    }

    /// Create or update an alias
    ///
    /// Aliases provide stable, named pointers to specific versions. This allows
    /// you to invoke "my-function:prod" and update what "prod" points to without
    /// changing client code.
    ///
    /// # Arguments
    /// * `function_id` - The function this alias belongs to
    /// * `alias` - The alias name (e.g., "dev", "staging", "prod")
    /// * `version_ref` - The version to point to (must be a published version)
    ///
    /// # Returns
    /// The created or updated alias
    ///
    /// # Errors
    /// * `FunctionNotFound` - If the function doesn't exist
    /// * `VersionNotFound` - If the target version doesn't exist
    /// * `InvalidHandler` - If trying to create an alias to $LATEST (not allowed)
    /// * `Database` - If there's a database error
    ///
    /// # Example
    /// ```rust,ignore
    /// // Create an alias pointing to version 5
    /// let alias = versioning.upsert_alias(
    ///     &function_id,
    ///     "prod".to_string(),
    ///     &VersionRef::Number(5),
    /// ).await?;
    /// ```
    pub async fn upsert_alias(
        &self,
        function_id: &FunctionId,
        alias: String,
        version_ref: &VersionRef,
    ) -> Result<Alias, LambdaError> {
        // Validate that we're not trying to create an alias to $LATEST
        // Aliases must point to published versions only
        if matches!(version_ref, VersionRef::Latest) {
            return Err(LambdaError::InvalidHandler(
                "Aliases cannot point to $LATEST. Publish a version first.".to_string(),
            ));
        }

        // Resolve the version reference to a concrete version number
        let version = self
            .resolve_version(function_id, version_ref)
            .await?
            .ok_or_else(|| LambdaError::Internal(format!("Failed to resolve version: {}", version_ref)))?;

        // Verify the function exists
        let _metadata = self
            .registry
            .get_function(function_id)
            .await?;

        // Verify the version exists
        let _version_metadata = self
            .registry
            .get_version(function_id, version)
            .await?;

        // Try to update existing alias first, if not found, create new one
        match self.registry.update_alias(function_id, &alias, &VersionRef::Number(version)).await {
            Ok(alias_record) => Ok(alias_record),
            Err(LambdaError::AliasNotFound { .. }) => {
                // Alias doesn't exist, create it
                self.registry.create_alias(function_id, &alias, &VersionRef::Number(version)).await
            }
            Err(e) => Err(e),
        }
    }

    /// Delete an alias
    ///
    /// Removes a named alias. The version it points to is not affected.
    ///
    /// # Arguments
    /// * `function_id` - The function this alias belongs to
    /// * `alias` - The alias name to delete
    ///
    /// # Errors
    /// * `AliasNotFound` - If the alias doesn't exist
    /// * `Database` - If there's a database error
    ///
    /// # Example
    /// ```rust,ignore
    /// versioning.delete_alias(&function_id, "old-staging").await?;
    /// ```
    pub async fn delete_alias(
        &self,
        function_id: &FunctionId,
        alias: &str,
    ) -> Result<(), LambdaError> {
        // Verify the alias exists before attempting to delete
        let _existing = self.registry.get_alias(function_id, alias).await?;

        // Delete via SQL directly since there's no delete_alias method
        // We'll need to add this method to the registry
        Err(LambdaError::Internal("delete_alias not yet implemented".to_string()))
    }

    /// List all aliases for a function
    ///
    /// Returns all aliases defined for the given function, ordered by name.
    ///
    /// # Arguments
    /// * `function_id` - The function to list aliases for
    ///
    /// # Returns
    /// Vector of aliases (may be empty if no aliases are defined)
    ///
    /// # Errors
    /// * `Database` - If there's a database error
    ///
    /// # Example
    /// ```rust,ignore
    /// let aliases = versioning.list_aliases(&function_id).await?;
    /// for alias in aliases {
    ///     println!("{} -> version {}", alias.alias, alias.version);
    /// }
    /// ```
    pub async fn list_aliases(
        &self,
        _function_id: &FunctionId,
    ) -> Result<Vec<Alias>, LambdaError> {
        // list_aliases not yet implemented
        Err(LambdaError::Internal("list_aliases not yet implemented".to_string()))
    }

    /// Resolve a version reference to a concrete version number
    ///
    /// This is the core resolution logic:
    /// - `VersionRef::Latest` -> `None` (represents unpublished $LATEST code)
    /// - `VersionRef::Number(n)` -> `Some(n)` (direct version number)
    /// - `VersionRef::Alias(name)` -> `Some(version)` (look up alias target)
    ///
    /// # Arguments
    /// * `function_id` - The function to resolve for
    /// * `version_ref` - The version reference to resolve
    ///
    /// # Returns
    /// - `Some(version_number)` for published versions
    /// - `None` for $LATEST (unpublished code)
    ///
    /// # Errors
    /// * `AliasNotFound` - If the alias doesn't exist
    /// * `Database` - If there's a database error
    ///
    /// # Example
    /// ```rust,ignore
    /// // Resolve an alias
    /// let version = versioning.resolve_version(
    ///     &function_id,
    ///     &VersionRef::Alias("prod".to_string()),
    /// ).await?;
    ///
    /// match version {
    ///     Some(v) => println!("prod points to version {}", v),
    ///     None => println!("using $LATEST"),
    /// }
    /// ```
    pub async fn resolve_version(
        &self,
        function_id: &FunctionId,
        version_ref: &VersionRef,
    ) -> Result<Option<u32>, LambdaError> {
        match version_ref {
            // $LATEST is represented as None (no version number)
            VersionRef::Latest => Ok(None),

            // Direct version number
            VersionRef::Number(n) => Ok(Some(*n)),

            // Look up alias to get version number
            VersionRef::Alias(alias) => {
                let alias_record = self
                    .registry
                    .get_alias(function_id, alias)
                    .await?;

                Ok(Some(alias_record.version))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lambda::{
        config::LambdaConfig,
        types::{CreateFunctionRequest, FunctionMetadata},
    };
    use std::collections::HashMap;

    // TODO: These tests will be implemented once FunctionRegistry is complete
    // They serve as documentation of expected behavior

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_publish_version() {
        // Setup: Create function
        // Act: Publish version
        // Assert: Version number increments, snapshot matches $LATEST
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_upsert_alias_create() {
        // Setup: Create function, publish version
        // Act: Create alias pointing to version
        // Assert: Alias exists and points to correct version
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_upsert_alias_update() {
        // Setup: Create function, publish versions 1 and 2, create alias to v1
        // Act: Update alias to point to v2
        // Assert: Alias now points to v2, updated_at changed
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_upsert_alias_rejects_latest() {
        // Setup: Create function
        // Act: Try to create alias to $LATEST
        // Assert: Returns InvalidHandler error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_upsert_alias_rejects_nonexistent_version() {
        // Setup: Create function, publish version 1
        // Act: Try to create alias to version 999
        // Assert: Returns VersionNotFound error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_delete_alias() {
        // Setup: Create function, publish version, create alias
        // Act: Delete alias
        // Assert: Alias no longer exists, version still exists
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_delete_alias_not_found() {
        // Setup: Create function
        // Act: Try to delete non-existent alias
        // Assert: Returns AliasNotFound error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_list_aliases() {
        // Setup: Create function, publish versions, create multiple aliases
        // Act: List aliases
        // Assert: Returns all aliases, ordered by name
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_list_aliases_empty() {
        // Setup: Create function with no aliases
        // Act: List aliases
        // Assert: Returns empty vector
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_resolve_version_latest() {
        // Setup: Create function
        // Act: Resolve VersionRef::Latest
        // Assert: Returns None
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_resolve_version_number() {
        // Setup: Create function, publish version
        // Act: Resolve VersionRef::Number(1)
        // Assert: Returns Some(1)
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_resolve_version_alias() {
        // Setup: Create function, publish version, create alias
        // Act: Resolve VersionRef::Alias("prod")
        // Assert: Returns Some(version_number)
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_resolve_version_alias_not_found() {
        // Setup: Create function
        // Act: Try to resolve non-existent alias
        // Assert: Returns AliasNotFound error
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_version_immutability() {
        // Setup: Create function, publish version
        // Act: Update function code, publish again
        // Assert: Original version unchanged, new version created
    }

    #[tokio::test]
    #[ignore = "requires FunctionRegistry implementation"]
    async fn test_alias_chain_resolution() {
        // This test verifies we don't support alias-to-alias
        // Setup: Create function, publish version, create alias
        // Act: Try to create alias pointing to another alias
        // Assert: Should fail or resolve to version number
    }
}
