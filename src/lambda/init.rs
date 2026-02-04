//! System initialization and migration runner
//!
//! This module provides the initialization layer that sets up the Lambda system:
//! - Database connection and migrations
//! - Component initialization (registry, runtime, invoker, etc.)
//! - System configuration

use crate::lambda::{
    config::LambdaConfig,
    invoker::FunctionInvoker,
    metrics::MetricsCollector,
    registry::FunctionRegistry,
    runtime::HandlerRuntime,
    types::LambdaError,
    versioning::VersioningManager,
};
use crate::runner::RunnerConfig;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Complete Lambda system with all components initialized
pub struct LambdaSystem {
    pub registry: Arc<FunctionRegistry>,
    pub runtime: Arc<HandlerRuntime>,
    pub invoker: Arc<FunctionInvoker>,
    pub versioning: Arc<VersioningManager>,
    pub metrics: Option<Arc<MetricsCollector>>,
    pub config: LambdaConfig,
    pool: SqlitePool,
}

impl LambdaSystem {
    /// Initialize the complete Lambda system
    ///
    /// This function:
    /// 1. Connects to the SQLite database
    /// 2. Runs all migrations
    /// 3. Initializes all components (registry, runtime, invoker, etc.)
    /// 4. Returns a fully configured LambdaSystem
    ///
    /// # Arguments
    /// * `config` - Lambda configuration
    ///
    /// # Returns
    /// Initialized LambdaSystem ready to use
    ///
    /// # Errors
    /// - Database connection errors
    /// - Migration errors
    /// - Component initialization errors
    ///
    /// # Example
    /// ```rust,no_run
    /// use jsexec::lambda::{LambdaConfig, LambdaSystem};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = LambdaConfig::new("sqlite:lambda.db");
    /// let system = LambdaSystem::initialize(config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn initialize(config: LambdaConfig) -> Result<Self, LambdaError> {
        info!("Initializing Lambda system with config: {:?}", config);

        // Step 1: Connect to database
        debug!("Connecting to database: {}", config.database_url);
        let pool = SqlitePool::connect(&config.database_url)
            .await
            .map_err(|e| {
                LambdaError::Internal(format!("Failed to connect to database: {}", e))
            })?;
        info!("Database connection established");

        // Step 2: Run migrations
        debug!("Running database migrations");
        run_migrations(&pool).await?;
        info!("Database migrations completed");

        // Step 3: Create FunctionRegistry
        debug!("Creating FunctionRegistry");
        let registry = Arc::new(FunctionRegistry::new(&config.database_url).await?);

        // Step 4: Create RunnerConfig for HandlerRuntime
        debug!("Creating RunnerConfig");
        let mut runner_config = RunnerConfig::default();

        // Set workerd binary path if configured
        if let Some(ref workerd_path) = config.workerd_path {
            runner_config.workerd_binary = workerd_path.clone();
        }

        // Lambda functions are self-contained, no preloaded modules needed
        runner_config.modules = Vec::new();

        // Step 5: Create HandlerRuntime
        debug!("Creating HandlerRuntime");
        let runtime = Arc::new(HandlerRuntime::new(runner_config));

        // Step 6: Create MetricsCollector (if enabled)
        let metrics = if config.enable_metrics {
            debug!("Creating MetricsCollector");
            Some(Arc::new(MetricsCollector::new(Arc::new(pool.clone())).await))
        } else {
            warn!("Metrics collection disabled");
            None
        };

        // Step 7: Create VersioningManager
        debug!("Creating VersioningManager");
        let versioning = Arc::new(VersioningManager::new(registry.clone()));

        // Step 8: Create FunctionInvoker
        debug!("Creating FunctionInvoker");
        let invoker = Arc::new(FunctionInvoker::new(
            registry.clone(),
            runtime.clone(),
            metrics.clone(),
        ));

        info!("Lambda system initialization complete");

        Ok(Self {
            registry,
            runtime,
            invoker,
            versioning,
            metrics,
            config,
            pool,
        })
    }

    /// Get a reference to the database pool
    ///
    /// This is useful for running custom queries or for testing.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Run database migrations
///
/// This function reads SQL files from the /workspace/migrations/ directory
/// and executes them in order to set up the database schema.
///
/// All migrations are idempotent (using CREATE TABLE IF NOT EXISTS),
/// so this function can be called multiple times safely.
///
/// # Arguments
/// * `pool` - SQLite connection pool
///
/// # Returns
/// Ok if all migrations succeeded
///
/// # Errors
/// Returns LambdaError::Database if any migration fails
///
/// # Migration order
/// 1. 001_create_functions_table.sql - Main functions table
/// 2. 002_create_versions_table.sql - Function versions
/// 3. 003_create_aliases_table.sql - Function aliases
/// 4. 004_create_invocations_table.sql - Invocation metrics
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), LambdaError> {
    info!("Running database migrations");

    // Embed migration files at compile time
    const MIGRATION_001: &str = include_str!("../../migrations/001_create_functions_table.sql");
    const MIGRATION_002: &str = include_str!("../../migrations/002_create_versions_table.sql");
    const MIGRATION_003: &str = include_str!("../../migrations/003_create_aliases_table.sql");
    const MIGRATION_004: &str = include_str!("../../migrations/004_create_invocations_table.sql");

    let migrations = vec![
        ("001_create_functions_table", MIGRATION_001),
        ("002_create_versions_table", MIGRATION_002),
        ("003_create_aliases_table", MIGRATION_003),
        ("004_create_invocations_table", MIGRATION_004),
    ];

    for (name, sql) in migrations {
        debug!("Running migration: {}", name);

        sqlx::query(sql)
            .execute(pool)
            .await
            .map_err(|e| {
                LambdaError::Database(e)
            })?;

        info!("Migration {} completed successfully", name);
    }

    info!("All migrations completed successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_migrations() {
        // Use in-memory database for testing
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to connect to in-memory database");

        // Run migrations
        run_migrations(&pool)
            .await
            .expect("Migrations should succeed");

        // Verify tables exist by querying them
        let result = sqlx::query("SELECT COUNT(*) FROM functions")
            .execute(&pool)
            .await;
        assert!(result.is_ok(), "functions table should exist");

        let result = sqlx::query("SELECT COUNT(*) FROM function_versions")
            .execute(&pool)
            .await;
        assert!(result.is_ok(), "function_versions table should exist");

        let result = sqlx::query("SELECT COUNT(*) FROM function_aliases")
            .execute(&pool)
            .await;
        assert!(result.is_ok(), "function_aliases table should exist");

        let result = sqlx::query("SELECT COUNT(*) FROM invocations")
            .execute(&pool)
            .await;
        assert!(result.is_ok(), "invocations table should exist");
    }

    #[tokio::test]
    async fn test_migrations_are_idempotent() {
        // Use in-memory database for testing
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to connect to in-memory database");

        // Run migrations twice
        run_migrations(&pool)
            .await
            .expect("First migration run should succeed");

        run_migrations(&pool)
            .await
            .expect("Second migration run should succeed (idempotent)");

        // Verify tables still exist and are functional
        let result = sqlx::query("SELECT COUNT(*) FROM functions")
            .execute(&pool)
            .await;
        assert!(result.is_ok(), "functions table should still exist");
    }

    #[tokio::test]
    async fn test_initialize_lambda_system() {
        // Use in-memory database for testing
        let config = LambdaConfig::in_memory();

        let system = LambdaSystem::initialize(config.clone())
            .await
            .expect("System initialization should succeed");

        // Verify all components are initialized
        assert!(!system.pool.is_closed(), "Pool should not be closed");
        assert!(system.metrics.is_some(), "Metrics should be enabled by default");
        assert_eq!(system.config.database_url, config.database_url);

        // Verify database has tables
        let pool = system.pool();
        let result = sqlx::query("SELECT COUNT(*) FROM functions")
            .execute(pool)
            .await;
        assert!(result.is_ok(), "Database should be initialized");
    }

    #[tokio::test]
    async fn test_initialize_without_metrics() {
        let config = LambdaConfig::in_memory().without_metrics();

        let system = LambdaSystem::initialize(config)
            .await
            .expect("System initialization should succeed");

        assert!(system.metrics.is_none(), "Metrics should be disabled");
    }

    #[tokio::test]
    async fn test_pool_reference() {
        let config = LambdaConfig::in_memory();
        let system = LambdaSystem::initialize(config)
            .await
            .expect("System initialization should succeed");

        let pool = system.pool();

        // Verify we can use the pool reference
        let result = sqlx::query("SELECT 1")
            .execute(pool)
            .await;
        assert!(result.is_ok(), "Pool should be usable");
    }
}
