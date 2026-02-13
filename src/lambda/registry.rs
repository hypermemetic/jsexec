//! Function Registry - Persistent storage for Lambda functions
//!
//! This module provides CRUD operations for functions, versions, and aliases
//! using SQLite as the backing store.

use crate::lambda::types::{
    Alias, CreateFunctionRequest, FunctionId, FunctionMetadata, FunctionVersion, LambdaError,
    UpdateFunctionRequest, VersionRef,
};
use sha2::{Digest, Sha256};
use sqlx::{sqlite::SqlitePool, Row};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Resolved version information (for execution)
#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    /// Version number (None for $LATEST)
    pub version: Option<u32>,
    /// Function code
    pub code: String,
    /// Handler name
    pub handler: String,
    /// Environment variables
    pub environment: HashMap<String, String>,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Memory limit in MB
    pub memory_mb: u64,
}

/// Function registry for managing Lambda functions in SQLite
pub struct FunctionRegistry {
    pool: Arc<SqlitePool>,
}

impl FunctionRegistry {
    /// Create a new function registry
    ///
    /// # Arguments
    /// * `database_url` - SQLite database URL (e.g., "sqlite:plambda.db")
    ///
    /// # Errors
    /// Returns an error if the database connection fails
    pub async fn new(database_url: &str) -> Result<Self, LambdaError> {
        let pool = SqlitePool::connect(database_url)
            .await
            .map_err(LambdaError::Database)?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    /// Get the current Unix timestamp in milliseconds
    fn current_timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Calculate SHA-256 hash of code
    fn calculate_code_hash(code: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(code.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Serialize environment variables to JSON
    fn serialize_env(env: &HashMap<String, String>) -> Result<String, LambdaError> {
        serde_json::to_string(env).map_err(|e| LambdaError::Internal(e.to_string()))
    }

    /// Deserialize environment variables from JSON
    fn deserialize_env(json: Option<&str>) -> Result<HashMap<String, String>, LambdaError> {
        match json {
            Some(s) if !s.is_empty() => {
                serde_json::from_str(s).map_err(|e| LambdaError::Internal(e.to_string()))
            }
            _ => Ok(HashMap::new()),
        }
    }

    /// Validate function name (alphanumeric, hyphens, underscores only)
    fn validate_name(name: &str) -> Result<(), LambdaError> {
        if name.is_empty() {
            return Err(LambdaError::InvalidName("name cannot be empty".to_string()));
        }
        if name.len() > 64 {
            return Err(LambdaError::InvalidName(
                "name cannot exceed 64 characters".to_string(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(LambdaError::InvalidName(
                "name can only contain alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            ));
        }
        Ok(())
    }

    // =========================================================================
    // CRUD Operations
    // =========================================================================

    /// Create a new function
    ///
    /// # Arguments
    /// * `req` - Function creation request
    ///
    /// # Errors
    /// Returns an error if:
    /// - The name is invalid or already exists
    /// - Database operation fails
    pub async fn create_function(
        &self,
        req: CreateFunctionRequest,
    ) -> Result<FunctionMetadata, LambdaError> {
        Self::validate_name(&req.name)?;

        let id = FunctionId::new();
        let code_hash = Self::calculate_code_hash(&req.code);
        let env_json = Self::serialize_env(&req.environment)?;
        let now = Self::current_timestamp_ms();

        let result = sqlx::query(
            r#"
            INSERT INTO functions (
                id, name, description, runtime, handler, code, code_hash,
                environment_variables, timeout_ms, memory_mb, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(&req.name)
        .bind(&req.description)
        .bind(&req.runtime)
        .bind(&req.handler)
        .bind(&req.code)
        .bind(&code_hash)
        .bind(&env_json)
        .bind(req.timeout_ms as i64)
        .bind(req.memory_mb as i64)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&*self.pool)
        .await;

        match result {
            Ok(_) => Ok(FunctionMetadata {
                id,
                name: req.name,
                description: req.description,
                runtime: req.runtime,
                handler: req.handler,
                code: req.code,
                code_hash,
                environment: req.environment,
                timeout_ms: req.timeout_ms,
                memory_mb: req.memory_mb,
                created_at: now,
                updated_at: now,
            }),
            Err(e) if e.to_string().contains("UNIQUE constraint failed") => {
                Err(LambdaError::NameAlreadyExists(req.name))
            }
            Err(e) => Err(LambdaError::Database(e)),
        }
    }

    /// Get function by ID
    ///
    /// # Arguments
    /// * `id` - Function ID
    ///
    /// # Errors
    /// Returns an error if the function is not found or database operation fails
    pub async fn get_function(&self, id: &FunctionId) -> Result<FunctionMetadata, LambdaError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, description, runtime, handler, code, code_hash,
                   environment_variables, timeout_ms, memory_mb, created_at, updated_at
            FROM functions
            WHERE id = ?
            "#,
        )
        .bind(id.to_string())
        .fetch_optional(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        match row {
            Some(row) => {
                let env_json: Option<String> = row.try_get("environment_variables").ok();
                let environment = Self::deserialize_env(env_json.as_deref())?;

                Ok(FunctionMetadata {
                    id: row.try_get::<String, _>("id")?.parse().unwrap(),
                    name: row.try_get("name")?,
                    description: row.try_get("description")?,
                    runtime: row.try_get("runtime")?,
                    handler: row.try_get("handler")?,
                    code: row.try_get("code")?,
                    code_hash: row.try_get("code_hash")?,
                    environment,
                    timeout_ms: row.try_get::<i64, _>("timeout_ms")? as u64,
                    memory_mb: row.try_get::<i64, _>("memory_mb")? as u64,
                    created_at: row.try_get::<i64, _>("created_at")? as u64,
                    updated_at: row.try_get::<i64, _>("updated_at")? as u64,
                })
            }
            None => Err(LambdaError::FunctionNotFound(id.to_string())),
        }
    }

    /// Get function by name
    ///
    /// # Arguments
    /// * `name` - Function name
    ///
    /// # Errors
    /// Returns an error if the function is not found or database operation fails
    pub async fn get_function_by_name(&self, name: &str) -> Result<FunctionMetadata, LambdaError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, description, runtime, handler, code, code_hash,
                   environment_variables, timeout_ms, memory_mb, created_at, updated_at
            FROM functions
            WHERE name = ?
            "#,
        )
        .bind(name)
        .fetch_optional(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        match row {
            Some(row) => {
                let env_json: Option<String> = row.try_get("environment_variables").ok();
                let environment = Self::deserialize_env(env_json.as_deref())?;

                Ok(FunctionMetadata {
                    id: row.try_get::<String, _>("id")?.parse().unwrap(),
                    name: row.try_get("name")?,
                    description: row.try_get("description")?,
                    runtime: row.try_get("runtime")?,
                    handler: row.try_get("handler")?,
                    code: row.try_get("code")?,
                    code_hash: row.try_get("code_hash")?,
                    environment,
                    timeout_ms: row.try_get::<i64, _>("timeout_ms")? as u64,
                    memory_mb: row.try_get::<i64, _>("memory_mb")? as u64,
                    created_at: row.try_get::<i64, _>("created_at")? as u64,
                    updated_at: row.try_get::<i64, _>("updated_at")? as u64,
                })
            }
            None => Err(LambdaError::FunctionNotFound(name.to_string())),
        }
    }

    /// List all functions
    ///
    /// # Errors
    /// Returns an error if database operation fails
    pub async fn list_functions(&self) -> Result<Vec<FunctionMetadata>, LambdaError> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, description, runtime, handler, code, code_hash,
                   environment_variables, timeout_ms, memory_mb, created_at, updated_at
            FROM functions
            ORDER BY name
            "#,
        )
        .fetch_all(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        let mut functions = Vec::new();
        for row in rows {
            let env_json: Option<String> = row.try_get("environment_variables").ok();
            let environment = Self::deserialize_env(env_json.as_deref())?;

            functions.push(FunctionMetadata {
                id: row.try_get::<String, _>("id")?.parse().unwrap(),
                name: row.try_get("name")?,
                description: row.try_get("description")?,
                runtime: row.try_get("runtime")?,
                handler: row.try_get("handler")?,
                code: row.try_get("code")?,
                code_hash: row.try_get("code_hash")?,
                environment,
                timeout_ms: row.try_get::<i64, _>("timeout_ms")? as u64,
                memory_mb: row.try_get::<i64, _>("memory_mb")? as u64,
                created_at: row.try_get::<i64, _>("created_at")? as u64,
                updated_at: row.try_get::<i64, _>("updated_at")? as u64,
            });
        }

        Ok(functions)
    }

    /// Update an existing function
    ///
    /// # Arguments
    /// * `id` - Function ID
    /// * `req` - Update request with optional fields
    ///
    /// # Errors
    /// Returns an error if the function is not found or database operation fails
    pub async fn update_function(
        &self,
        id: &FunctionId,
        req: UpdateFunctionRequest,
    ) -> Result<FunctionMetadata, LambdaError> {
        // Get current function to merge updates
        let mut current = self.get_function(id).await?;

        // Apply updates
        if let Some(description) = req.description {
            current.description = Some(description);
        }
        if let Some(handler) = req.handler {
            current.handler = handler;
        }
        if let Some(code) = req.code {
            current.code = code;
            current.code_hash = Self::calculate_code_hash(&current.code);
        }
        if let Some(environment) = req.environment {
            current.environment = environment;
        }
        if let Some(timeout_ms) = req.timeout_ms {
            current.timeout_ms = timeout_ms;
        }
        if let Some(memory_mb) = req.memory_mb {
            current.memory_mb = memory_mb;
        }

        let env_json = Self::serialize_env(&current.environment)?;
        let now = Self::current_timestamp_ms();
        current.updated_at = now;

        sqlx::query(
            r#"
            UPDATE functions
            SET description = ?, handler = ?, code = ?, code_hash = ?,
                environment_variables = ?, timeout_ms = ?, memory_mb = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&current.description)
        .bind(&current.handler)
        .bind(&current.code)
        .bind(&current.code_hash)
        .bind(&env_json)
        .bind(current.timeout_ms as i64)
        .bind(current.memory_mb as i64)
        .bind(now as i64)
        .bind(id.to_string())
        .execute(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        Ok(current)
    }

    /// Delete a function
    ///
    /// # Arguments
    /// * `id` - Function ID
    ///
    /// # Errors
    /// Returns an error if the function is not found or database operation fails
    ///
    /// # Note
    /// This will cascade delete all versions and aliases due to foreign key constraints
    pub async fn delete_function(&self, id: &FunctionId) -> Result<(), LambdaError> {
        let result = sqlx::query(
            r#"
            DELETE FROM functions
            WHERE id = ?
            "#,
        )
        .bind(id.to_string())
        .execute(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        if result.rows_affected() == 0 {
            return Err(LambdaError::FunctionNotFound(id.to_string()));
        }

        Ok(())
    }

    // =========================================================================
    // Versioning
    // =========================================================================

    /// Publish a new version by snapshotting current function state
    ///
    /// # Arguments
    /// * `id` - Function ID
    ///
    /// # Errors
    /// Returns an error if the function is not found or database operation fails
    pub async fn publish_version(
        &self,
        id: &FunctionId,
    ) -> Result<FunctionVersion, LambdaError> {
        let function = self.get_function(id).await?;

        // Get next version number
        let next_version = self.get_next_version_number(id).await?;

        let env_json = Self::serialize_env(&function.environment)?;
        let now = Self::current_timestamp_ms();

        sqlx::query(
            r#"
            INSERT INTO function_versions (
                function_id, version, code, code_hash, handler,
                environment_variables, timeout_ms, memory_mb, published_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(next_version as i64)
        .bind(&function.code)
        .bind(&function.code_hash)
        .bind(&function.handler)
        .bind(&env_json)
        .bind(function.timeout_ms as i64)
        .bind(function.memory_mb as i64)
        .bind(now as i64)
        .execute(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        Ok(FunctionVersion {
            function_id: *id,
            version: next_version,
            code: function.code,
            code_hash: function.code_hash,
            handler: function.handler,
            environment: function.environment,
            timeout_ms: function.timeout_ms,
            memory_mb: function.memory_mb,
            published_at: now,
        })
    }

    /// Get a specific published version
    ///
    /// # Arguments
    /// * `id` - Function ID
    /// * `version` - Version number
    ///
    /// # Errors
    /// Returns an error if the version is not found or database operation fails
    pub async fn get_version(
        &self,
        id: &FunctionId,
        version: u32,
    ) -> Result<FunctionVersion, LambdaError> {
        let row = sqlx::query(
            r#"
            SELECT function_id, version, code, code_hash, handler,
                   environment_variables, timeout_ms, memory_mb, published_at
            FROM function_versions
            WHERE function_id = ? AND version = ?
            "#,
        )
        .bind(id.to_string())
        .bind(version as i64)
        .fetch_optional(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        match row {
            Some(row) => {
                let env_json: Option<String> = row.try_get("environment_variables").ok();
                let environment = Self::deserialize_env(env_json.as_deref())?;

                Ok(FunctionVersion {
                    function_id: *id,
                    version: row.try_get::<i64, _>("version")? as u32,
                    code: row.try_get("code")?,
                    code_hash: row.try_get("code_hash")?,
                    handler: row.try_get("handler")?,
                    environment,
                    timeout_ms: row.try_get::<i64, _>("timeout_ms")? as u64,
                    memory_mb: row.try_get::<i64, _>("memory_mb")? as u64,
                    published_at: row.try_get::<i64, _>("published_at")? as u64,
                })
            }
            None => Err(LambdaError::VersionNotFound {
                function_id: *id,
                version,
            }),
        }
    }

    /// List all published versions for a function
    ///
    /// # Arguments
    /// * `id` - Function ID
    ///
    /// # Errors
    /// Returns an error if database operation fails
    pub async fn list_versions(
        &self,
        id: &FunctionId,
    ) -> Result<Vec<FunctionVersion>, LambdaError> {
        let rows = sqlx::query(
            r#"
            SELECT function_id, version, code, code_hash, handler,
                   environment_variables, timeout_ms, memory_mb, published_at
            FROM function_versions
            WHERE function_id = ?
            ORDER BY version DESC
            "#,
        )
        .bind(id.to_string())
        .fetch_all(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        let mut versions = Vec::new();
        for row in rows {
            let env_json: Option<String> = row.try_get("environment_variables").ok();
            let environment = Self::deserialize_env(env_json.as_deref())?;

            versions.push(FunctionVersion {
                function_id: *id,
                version: row.try_get::<i64, _>("version")? as u32,
                code: row.try_get("code")?,
                code_hash: row.try_get("code_hash")?,
                handler: row.try_get("handler")?,
                environment,
                timeout_ms: row.try_get::<i64, _>("timeout_ms")? as u64,
                memory_mb: row.try_get::<i64, _>("memory_mb")? as u64,
                published_at: row.try_get::<i64, _>("published_at")? as u64,
            });
        }

        Ok(versions)
    }

    /// Get the next version number for a function
    async fn get_next_version_number(&self, id: &FunctionId) -> Result<u32, LambdaError> {
        let row = sqlx::query(
            r#"
            SELECT MAX(version) as max_version
            FROM function_versions
            WHERE function_id = ?
            "#,
        )
        .bind(id.to_string())
        .fetch_one(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        let max_version: Option<i64> = row.try_get("max_version").ok().flatten();
        Ok(max_version.map(|v| v as u32 + 1).unwrap_or(1))
    }

    // =========================================================================
    // Aliases
    // =========================================================================

    /// Create a new alias pointing to a specific version
    ///
    /// # Arguments
    /// * `id` - Function ID
    /// * `alias` - Alias name
    /// * `version_ref` - Version reference (must be a specific number, not $LATEST)
    ///
    /// # Errors
    /// Returns an error if:
    /// - The version reference is $LATEST
    /// - The version doesn't exist
    /// - The alias already exists
    /// - Database operation fails
    pub async fn create_alias(
        &self,
        id: &FunctionId,
        alias: &str,
        version_ref: &VersionRef,
    ) -> Result<Alias, LambdaError> {
        let version = match version_ref {
            VersionRef::Number(v) => *v,
            VersionRef::Latest => {
                return Err(LambdaError::Internal(
                    "aliases cannot point to $LATEST".to_string(),
                ))
            }
            VersionRef::Alias(_) => {
                return Err(LambdaError::Internal(
                    "aliases cannot point to other aliases".to_string(),
                ))
            }
        };

        // Verify the version exists
        self.get_version(id, version).await?;

        let now = Self::current_timestamp_ms();

        let result = sqlx::query(
            r#"
            INSERT INTO function_aliases (function_id, alias, version, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(alias)
        .bind(version as i64)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&*self.pool)
        .await;

        match result {
            Ok(_) => Ok(Alias {
                function_id: *id,
                alias: alias.to_string(),
                version,
                created_at: now,
                updated_at: now,
            }),
            Err(e) if e.to_string().contains("UNIQUE constraint failed") => {
                Err(LambdaError::Internal(format!(
                    "alias '{}' already exists",
                    alias
                )))
            }
            Err(e) => Err(LambdaError::Database(e)),
        }
    }

    /// Update an existing alias to point to a different version
    ///
    /// # Arguments
    /// * `id` - Function ID
    /// * `alias` - Alias name
    /// * `version_ref` - New version reference (must be a specific number, not $LATEST)
    ///
    /// # Errors
    /// Returns an error if:
    /// - The version reference is $LATEST
    /// - The version doesn't exist
    /// - The alias doesn't exist
    /// - Database operation fails
    pub async fn update_alias(
        &self,
        id: &FunctionId,
        alias: &str,
        version_ref: &VersionRef,
    ) -> Result<Alias, LambdaError> {
        let version = match version_ref {
            VersionRef::Number(v) => *v,
            VersionRef::Latest => {
                return Err(LambdaError::Internal(
                    "aliases cannot point to $LATEST".to_string(),
                ))
            }
            VersionRef::Alias(_) => {
                return Err(LambdaError::Internal(
                    "aliases cannot point to other aliases".to_string(),
                ))
            }
        };

        // Verify the version exists
        self.get_version(id, version).await?;

        // Get current alias to preserve created_at
        let current = self.get_alias(id, alias).await?;

        let now = Self::current_timestamp_ms();

        sqlx::query(
            r#"
            UPDATE function_aliases
            SET version = ?, updated_at = ?
            WHERE function_id = ? AND alias = ?
            "#,
        )
        .bind(version as i64)
        .bind(now as i64)
        .bind(id.to_string())
        .bind(alias)
        .execute(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        Ok(Alias {
            function_id: *id,
            alias: alias.to_string(),
            version,
            created_at: current.created_at,
            updated_at: now,
        })
    }

    /// Get an alias
    ///
    /// # Arguments
    /// * `id` - Function ID
    /// * `alias` - Alias name
    ///
    /// # Errors
    /// Returns an error if the alias is not found or database operation fails
    pub async fn get_alias(&self, id: &FunctionId, alias: &str) -> Result<Alias, LambdaError> {
        let row = sqlx::query(
            r#"
            SELECT function_id, alias, version, created_at, updated_at
            FROM function_aliases
            WHERE function_id = ? AND alias = ?
            "#,
        )
        .bind(id.to_string())
        .bind(alias)
        .fetch_optional(&*self.pool)
        .await
        .map_err(LambdaError::Database)?;

        match row {
            Some(row) => Ok(Alias {
                function_id: *id,
                alias: row.try_get("alias")?,
                version: row.try_get::<i64, _>("version")? as u32,
                created_at: row.try_get::<i64, _>("created_at")? as u64,
                updated_at: row.try_get::<i64, _>("updated_at")? as u64,
            }),
            None => Err(LambdaError::AliasNotFound {
                function_id: *id,
                alias: alias.to_string(),
            }),
        }
    }

    /// Resolve a version reference to concrete execution data
    ///
    /// # Arguments
    /// * `id` - Function ID
    /// * `version_ref` - Version reference ($LATEST, version number, or alias)
    ///
    /// # Errors
    /// Returns an error if:
    /// - The function is not found
    /// - The version/alias is not found
    /// - Database operation fails
    pub async fn resolve_version(
        &self,
        id: &FunctionId,
        version_ref: &VersionRef,
    ) -> Result<ResolvedVersion, LambdaError> {
        match version_ref {
            VersionRef::Latest => {
                // Get current function state
                let function = self.get_function(id).await?;
                Ok(ResolvedVersion {
                    version: None,
                    code: function.code,
                    handler: function.handler,
                    environment: function.environment,
                    timeout_ms: function.timeout_ms,
                    memory_mb: function.memory_mb,
                })
            }
            VersionRef::Number(v) => {
                // Get specific version
                let version = self.get_version(id, *v).await?;
                Ok(ResolvedVersion {
                    version: Some(version.version),
                    code: version.code,
                    handler: version.handler,
                    environment: version.environment,
                    timeout_ms: version.timeout_ms,
                    memory_mb: version.memory_mb,
                })
            }
            VersionRef::Alias(a) => {
                // Resolve alias to version number
                let alias = self.get_alias(id, a).await?;
                let version = self.get_version(id, alias.version).await?;
                Ok(ResolvedVersion {
                    version: Some(version.version),
                    code: version.code,
                    handler: version.handler,
                    environment: version.environment,
                    timeout_ms: version.timeout_ms,
                    memory_mb: version.memory_mb,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_registry() -> FunctionRegistry {
        let registry = FunctionRegistry::new("sqlite::memory:")
            .await
            .expect("Failed to create registry");

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&*registry.pool)
            .await
            .expect("Failed to run migrations");

        registry
    }

    fn create_test_function_request(name: &str) -> CreateFunctionRequest {
        CreateFunctionRequest {
            name: name.to_string(),
            description: Some("Test function".to_string()),
            runtime: "javascript".to_string(),
            handler: "handler".to_string(),
            code: "export function handler(event) { return event; }".to_string(),
            environment: HashMap::from([("KEY".to_string(), "value".to_string())]),
            timeout_ms: 30000,
            memory_mb: 128,
        }
    }

    #[tokio::test]
    async fn test_create_and_get_function() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("test-func");

        let created = registry
            .create_function(req.clone())
            .await
            .expect("Failed to create function");

        assert_eq!(created.name, "test-func");
        assert_eq!(created.code, req.code);
        assert_eq!(created.environment.get("KEY").unwrap(), "value");

        let retrieved = registry
            .get_function(&created.id)
            .await
            .expect("Failed to get function");

        assert_eq!(retrieved.id, created.id);
        assert_eq!(retrieved.name, created.name);
    }

    #[tokio::test]
    async fn test_duplicate_name_error() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("duplicate");

        registry
            .create_function(req.clone())
            .await
            .expect("Failed to create first function");

        let result = registry.create_function(req).await;
        assert!(matches!(result, Err(LambdaError::NameAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_get_function_by_name() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("named-func");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let retrieved = registry
            .get_function_by_name("named-func")
            .await
            .expect("Failed to get function by name");

        assert_eq!(retrieved.id, created.id);
    }

    #[tokio::test]
    async fn test_update_function() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("update-func");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let update_req = UpdateFunctionRequest {
            code: Some("export function handler() { return 'updated'; }".to_string()),
            handler: Some("newHandler".to_string()),
            ..Default::default()
        };

        let updated = registry
            .update_function(&created.id, update_req)
            .await
            .expect("Failed to update function");

        assert_eq!(updated.handler, "newHandler");
        assert!(updated.code.contains("updated"));
        assert_ne!(updated.code_hash, created.code_hash);
    }

    #[tokio::test]
    async fn test_list_functions() {
        let registry = create_test_registry().await;

        registry
            .create_function(create_test_function_request("func1"))
            .await
            .expect("Failed to create func1");

        registry
            .create_function(create_test_function_request("func2"))
            .await
            .expect("Failed to create func2");

        let list = registry
            .list_functions()
            .await
            .expect("Failed to list functions");

        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_function() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("delete-func");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        registry
            .delete_function(&created.id)
            .await
            .expect("Failed to delete function");

        let result = registry.get_function(&created.id).await;
        assert!(matches!(result, Err(LambdaError::FunctionNotFound(_))));
    }

    #[tokio::test]
    async fn test_publish_version() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("version-func");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let v1 = registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish v1");

        assert_eq!(v1.version, 1);
        assert_eq!(v1.code, created.code);

        // Update function and publish again
        let update_req = UpdateFunctionRequest {
            code: Some("export function handler() { return 'v2'; }".to_string()),
            ..Default::default()
        };

        registry
            .update_function(&created.id, update_req)
            .await
            .expect("Failed to update function");

        let v2 = registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish v2");

        assert_eq!(v2.version, 2);
        assert_ne!(v1.code_hash, v2.code_hash);
    }

    #[tokio::test]
    async fn test_list_versions() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("multi-version");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish v1");

        registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish v2");

        let versions = registry
            .list_versions(&created.id)
            .await
            .expect("Failed to list versions");

        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 2); // Ordered DESC
        assert_eq!(versions[1].version, 1);
    }

    #[tokio::test]
    async fn test_create_and_get_alias() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("alias-func");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let version = registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish version");

        let alias = registry
            .create_alias(&created.id, "prod", &VersionRef::Number(version.version))
            .await
            .expect("Failed to create alias");

        assert_eq!(alias.alias, "prod");
        assert_eq!(alias.version, 1);

        let retrieved = registry
            .get_alias(&created.id, "prod")
            .await
            .expect("Failed to get alias");

        assert_eq!(retrieved.version, alias.version);
    }

    #[tokio::test]
    async fn test_update_alias() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("alias-update");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish v1");

        registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish v2");

        registry
            .create_alias(&created.id, "staging", &VersionRef::Number(1))
            .await
            .expect("Failed to create alias");

        let updated = registry
            .update_alias(&created.id, "staging", &VersionRef::Number(2))
            .await
            .expect("Failed to update alias");

        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn test_resolve_latest() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("resolve-latest");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let resolved = registry
            .resolve_version(&created.id, &VersionRef::Latest)
            .await
            .expect("Failed to resolve $LATEST");

        assert_eq!(resolved.version, None);
        assert_eq!(resolved.code, created.code);
    }

    #[tokio::test]
    async fn test_resolve_version_number() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("resolve-number");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let version = registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish version");

        let resolved = registry
            .resolve_version(&created.id, &VersionRef::Number(1))
            .await
            .expect("Failed to resolve version 1");

        assert_eq!(resolved.version, Some(1));
        assert_eq!(resolved.code, version.code);
    }

    #[tokio::test]
    async fn test_resolve_alias() {
        let registry = create_test_registry().await;
        let req = create_test_function_request("resolve-alias");

        let created = registry
            .create_function(req)
            .await
            .expect("Failed to create function");

        let version = registry
            .publish_version(&created.id)
            .await
            .expect("Failed to publish version");

        registry
            .create_alias(&created.id, "prod", &VersionRef::Number(1))
            .await
            .expect("Failed to create alias");

        let resolved = registry
            .resolve_version(&created.id, &VersionRef::Alias("prod".to_string()))
            .await
            .expect("Failed to resolve alias");

        assert_eq!(resolved.version, Some(1));
        assert_eq!(resolved.code, version.code);
    }

    #[tokio::test]
    async fn test_code_hash_calculation() {
        let code1 = "export function handler() { return 1; }";
        let code2 = "export function handler() { return 2; }";

        let hash1 = FunctionRegistry::calculate_code_hash(code1);
        let hash2 = FunctionRegistry::calculate_code_hash(code2);

        assert_ne!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex characters
    }

    #[tokio::test]
    async fn test_invalid_function_name() {
        let registry = create_test_registry().await;
        let mut req = create_test_function_request("invalid name!");

        req.name = "invalid name!".to_string();
        let result = registry.create_function(req).await;
        assert!(matches!(result, Err(LambdaError::InvalidName(_))));
    }
}
