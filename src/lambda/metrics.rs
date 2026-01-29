//! Metrics collection for Lambda invocations
//!
//! This module provides comprehensive metrics tracking for function invocations,
//! including aggregated statistics, percentile calculations, and historical data.

use crate::lambda::types::{FunctionId, FunctionMetrics, InvocationRecord, LambdaError};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Collector for Lambda function invocation metrics
///
/// Stores invocation records in SQLite and provides aggregated metrics
/// over time ranges, including percentile calculations and success rates.
pub struct MetricsCollector {
    pool: Arc<SqlitePool>,
}

impl MetricsCollector {
    /// Create a new metrics collector with the given database pool
    pub async fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Record a single invocation in the database
    ///
    /// Stores all metrics from the invocation for later aggregation and analysis.
    pub async fn record_invocation(&self, record: InvocationRecord) -> Result<(), LambdaError> {
        sqlx::query(
            r#"
            INSERT INTO invocations (
                invocation_id,
                function_id,
                function_name,
                version,
                invocation_type,
                cold_start,
                duration_ms,
                memory_used_bytes,
                cpu_time_ms,
                success,
                error_message,
                error_type,
                started_at,
                completed_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(record.invocation_id.to_string())
        .bind(record.function_id.to_string())
        .bind(record.function_name)
        .bind(record.version.map(|v| v as i64))
        .bind(record.invocation_type.to_string())
        .bind(if record.cold_start { 1 } else { 0 })
        .bind(record.duration_ms.map(|d| d as i64))
        .bind(record.memory_used_bytes.map(|m| m as i64))
        .bind(record.cpu_time_ms.map(|c| c as i64))
        .bind(if record.success { 1 } else { 0 })
        .bind(record.error_message)
        .bind(record.error_type)
        .bind(record.started_at as i64)
        .bind(record.completed_at.map(|c| c as i64))
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    /// Get aggregated metrics for a function over a time range
    ///
    /// # Arguments
    /// * `function_id` - The function to get metrics for
    /// * `start_time` - Optional start time (Unix timestamp ms). If None, includes all history
    /// * `end_time` - Optional end time (Unix timestamp ms). If None, includes up to now
    ///
    /// # Returns
    /// Aggregated metrics including counts, success rates, and duration percentiles
    pub async fn get_function_metrics(
        &self,
        function_id: &FunctionId,
        start_time: Option<u64>,
        end_time: Option<u64>,
    ) -> Result<FunctionMetrics, LambdaError> {
        // Build the WHERE clause based on time constraints
        let mut conditions = vec!["function_id = ?".to_string()];
        if start_time.is_some() {
            conditions.push("started_at >= ?".to_string());
        }
        if end_time.is_some() {
            conditions.push("started_at <= ?".to_string());
        }
        let where_clause = conditions.join(" AND ");

        // Get aggregate counts and averages
        let query_str = format!(
            r#"
            SELECT
                COUNT(*) as total_invocations,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successful_invocations,
                SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) as failed_invocations,
                SUM(CASE WHEN cold_start = 1 THEN 1 ELSE 0 END) as cold_starts,
                AVG(CASE WHEN duration_ms IS NOT NULL THEN duration_ms ELSE 0 END) as avg_duration_ms
            FROM invocations
            WHERE {}
            "#,
            where_clause
        );

        let mut query = sqlx::query_as::<_, AggregateRow>(&query_str)
            .bind(function_id.to_string());

        if let Some(start) = start_time {
            query = query.bind(start as i64);
        }
        if let Some(end) = end_time {
            query = query.bind(end as i64);
        }

        let aggregate = query.fetch_one(&*self.pool).await?;

        // If no invocations, return zero metrics
        if aggregate.total_invocations == 0 {
            return Ok(FunctionMetrics {
                function_id: *function_id,
                total_invocations: 0,
                successful_invocations: 0,
                failed_invocations: 0,
                cold_starts: 0,
                avg_duration_ms: 0.0,
                p50_duration_ms: 0,
                p95_duration_ms: 0,
                p99_duration_ms: 0,
            });
        }

        // Calculate percentiles
        let percentiles = self
            .calculate_percentiles(function_id, start_time, end_time)
            .await?;

        Ok(FunctionMetrics {
            function_id: *function_id,
            total_invocations: aggregate.total_invocations as u64,
            successful_invocations: aggregate.successful_invocations as u64,
            failed_invocations: aggregate.failed_invocations as u64,
            cold_starts: aggregate.cold_starts as u64,
            avg_duration_ms: aggregate.avg_duration_ms,
            p50_duration_ms: percentiles.p50,
            p95_duration_ms: percentiles.p95,
            p99_duration_ms: percentiles.p99,
        })
    }

    /// Get recent invocations for a function
    ///
    /// Returns the most recent invocations, ordered by start time (newest first).
    ///
    /// # Arguments
    /// * `function_id` - The function to get invocations for
    /// * `limit` - Maximum number of invocations to return
    pub async fn get_recent_invocations(
        &self,
        function_id: &FunctionId,
        limit: usize,
    ) -> Result<Vec<InvocationRecord>, LambdaError> {
        let rows = sqlx::query_as::<_, InvocationRow>(
            r#"
            SELECT
                invocation_id,
                function_id,
                function_name,
                version,
                invocation_type,
                cold_start,
                duration_ms,
                memory_used_bytes,
                cpu_time_ms,
                success,
                error_message,
                error_type,
                started_at,
                completed_at
            FROM invocations
            WHERE function_id = ?
            ORDER BY started_at DESC
            LIMIT ?
            "#,
        )
        .bind(function_id.to_string())
        .bind(limit as i64)
        .fetch_all(&*self.pool)
        .await?;

        rows.into_iter().map(|row| row.try_into()).collect()
    }

    /// Get global statistics across all functions
    ///
    /// Returns aggregated metrics for all functions in the system.
    pub async fn get_global_stats(&self) -> Result<GlobalStats, LambdaError> {
        let aggregate = sqlx::query_as::<_, GlobalStatsRow>(
            r#"
            SELECT
                COUNT(*) as total_invocations,
                COUNT(DISTINCT function_id) as total_functions,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as successful_invocations,
                SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) as failed_invocations,
                SUM(CASE WHEN cold_start = 1 THEN 1 ELSE 0 END) as cold_starts,
                AVG(CASE WHEN duration_ms IS NOT NULL THEN duration_ms ELSE 0 END) as avg_duration_ms,
                SUM(CASE WHEN memory_used_bytes IS NOT NULL THEN memory_used_bytes ELSE 0 END) as total_memory_bytes
            FROM invocations
            "#,
        )
        .fetch_one(&*self.pool)
        .await?;

        Ok(GlobalStats {
            total_invocations: aggregate.total_invocations as u64,
            total_functions: aggregate.total_functions as u64,
            successful_invocations: aggregate.successful_invocations as u64,
            failed_invocations: aggregate.failed_invocations as u64,
            cold_starts: aggregate.cold_starts as u64,
            avg_duration_ms: aggregate.avg_duration_ms,
            total_memory_bytes: aggregate.total_memory_bytes as u64,
        })
    }

    /// Clean up old invocation records (retention policy)
    ///
    /// Deletes invocation records older than the specified retention period.
    ///
    /// # Arguments
    /// * `retention_ms` - How long to keep records (in milliseconds)
    ///
    /// # Returns
    /// The number of records deleted
    pub async fn cleanup_old_records(&self, retention_ms: u64) -> Result<u64, LambdaError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let cutoff = now.saturating_sub(retention_ms);

        let result = sqlx::query(
            r#"
            DELETE FROM invocations
            WHERE started_at < ?
            "#,
        )
        .bind(cutoff as i64)
        .execute(&*self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Calculate duration percentiles for a function
    ///
    /// Uses ORDER BY and LIMIT to approximate percentiles since SQLite
    /// doesn't have built-in percentile functions in all versions.
    async fn calculate_percentiles(
        &self,
        function_id: &FunctionId,
        start_time: Option<u64>,
        end_time: Option<u64>,
    ) -> Result<Percentiles, LambdaError> {
        // Build WHERE clause
        let mut conditions = vec!["function_id = ?".to_string(), "duration_ms IS NOT NULL".to_string()];
        if start_time.is_some() {
            conditions.push("started_at >= ?".to_string());
        }
        if end_time.is_some() {
            conditions.push("started_at <= ?".to_string());
        }
        let where_clause = conditions.join(" AND ");

        // Get total count of invocations with durations
        let count_query = format!(
            "SELECT COUNT(*) as count FROM invocations WHERE {}",
            where_clause
        );

        let mut query = sqlx::query_as::<_, CountRow>(&count_query)
            .bind(function_id.to_string());

        if let Some(start) = start_time {
            query = query.bind(start as i64);
        }
        if let Some(end) = end_time {
            query = query.bind(end as i64);
        }

        let count_row = query.fetch_one(&*self.pool).await?;
        let total_count = count_row.count;

        if total_count == 0 {
            return Ok(Percentiles {
                p50: 0,
                p95: 0,
                p99: 0,
            });
        }

        // Calculate offsets for each percentile
        let p50_offset = (total_count as f64 * 0.50) as i64;
        let p95_offset = (total_count as f64 * 0.95) as i64;
        let p99_offset = (total_count as f64 * 0.99) as i64;

        // Fetch p50
        let p50 = self
            .fetch_percentile(function_id, start_time, end_time, p50_offset)
            .await?;

        // Fetch p95
        let p95 = self
            .fetch_percentile(function_id, start_time, end_time, p95_offset)
            .await?;

        // Fetch p99
        let p99 = self
            .fetch_percentile(function_id, start_time, end_time, p99_offset)
            .await?;

        Ok(Percentiles { p50, p95, p99 })
    }

    /// Fetch a specific percentile value
    async fn fetch_percentile(
        &self,
        function_id: &FunctionId,
        start_time: Option<u64>,
        end_time: Option<u64>,
        offset: i64,
    ) -> Result<u64, LambdaError> {
        let mut conditions = vec!["function_id = ?".to_string(), "duration_ms IS NOT NULL".to_string()];
        if start_time.is_some() {
            conditions.push("started_at >= ?".to_string());
        }
        if end_time.is_some() {
            conditions.push("started_at <= ?".to_string());
        }
        let where_clause = conditions.join(" AND ");

        let query_str = format!(
            r#"
            SELECT duration_ms
            FROM invocations
            WHERE {}
            ORDER BY duration_ms
            LIMIT 1 OFFSET ?
            "#,
            where_clause
        );

        let mut query = sqlx::query_as::<_, DurationRow>(&query_str)
            .bind(function_id.to_string());

        if let Some(start) = start_time {
            query = query.bind(start as i64);
        }
        if let Some(end) = end_time {
            query = query.bind(end as i64);
        }

        query = query.bind(offset);

        let row = query.fetch_one(&*self.pool).await?;
        Ok(row.duration_ms as u64)
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

#[derive(sqlx::FromRow)]
struct AggregateRow {
    total_invocations: i64,
    successful_invocations: i64,
    failed_invocations: i64,
    cold_starts: i64,
    avg_duration_ms: f64,
}

#[derive(sqlx::FromRow)]
struct CountRow {
    count: i64,
}

#[derive(sqlx::FromRow)]
struct DurationRow {
    duration_ms: i64,
}

#[derive(sqlx::FromRow)]
struct InvocationRow {
    invocation_id: String,
    function_id: String,
    function_name: String,
    version: Option<i64>,
    invocation_type: String,
    cold_start: i64,
    duration_ms: Option<i64>,
    memory_used_bytes: Option<i64>,
    cpu_time_ms: Option<i64>,
    success: i64,
    error_message: Option<String>,
    error_type: Option<String>,
    started_at: i64,
    completed_at: Option<i64>,
}

impl TryFrom<InvocationRow> for InvocationRecord {
    type Error = LambdaError;

    fn try_from(row: InvocationRow) -> Result<Self, Self::Error> {
        use std::str::FromStr;

        Ok(InvocationRecord {
            invocation_id: row.invocation_id.parse().map_err(|e| {
                LambdaError::Internal(format!("Invalid invocation_id: {}", e))
            })?,
            function_id: FunctionId::from_str(&row.function_id).map_err(|e| {
                LambdaError::Internal(format!("Invalid function_id: {}", e))
            })?,
            function_name: row.function_name,
            version: row.version.map(|v| v as u32),
            invocation_type: row.invocation_type.parse().map_err(|e| {
                LambdaError::Internal(format!("Invalid invocation_type: {}", e))
            })?,
            cold_start: row.cold_start != 0,
            duration_ms: row.duration_ms.map(|d| d as u64),
            memory_used_bytes: row.memory_used_bytes.map(|m| m as u64),
            cpu_time_ms: row.cpu_time_ms.map(|c| c as u64),
            success: row.success != 0,
            error_message: row.error_message,
            error_type: row.error_type,
            started_at: row.started_at as u64,
            completed_at: row.completed_at.map(|c| c as u64),
        })
    }
}

#[derive(sqlx::FromRow)]
struct GlobalStatsRow {
    total_invocations: i64,
    total_functions: i64,
    successful_invocations: i64,
    failed_invocations: i64,
    cold_starts: i64,
    avg_duration_ms: f64,
    total_memory_bytes: i64,
}

// =============================================================================
// Helper Types
// =============================================================================

struct Percentiles {
    p50: u64,
    p95: u64,
    p99: u64,
}

/// Global statistics across all functions
#[derive(Debug, Clone)]
pub struct GlobalStats {
    /// Total invocations across all functions
    pub total_invocations: u64,
    /// Number of unique functions
    pub total_functions: u64,
    /// Total successful invocations
    pub successful_invocations: u64,
    /// Total failed invocations
    pub failed_invocations: u64,
    /// Total cold starts
    pub cold_starts: u64,
    /// Average duration across all invocations (ms)
    pub avg_duration_ms: f64,
    /// Total memory used across all invocations (bytes)
    pub total_memory_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lambda::types::{InvocationId, InvocationType};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_test_db() -> Arc<SqlitePool> {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .expect("Failed to create in-memory database");

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run migrations");

        Arc::new(pool)
    }

    async fn insert_test_function(pool: &SqlitePool, id: FunctionId, name: &str) {
        sqlx::query(
            r#"
            INSERT INTO functions (id, name, description, runtime, handler, code, code_hash, environment, timeout_ms, memory_mb, created_at, updated_at)
            VALUES (?, ?, '', 'javascript', 'handler', 'export default {}', 'hash', '{}', 30000, 128, 0, 0)
            "#,
        )
        .bind(id.to_string())
        .bind(name)
        .execute(pool)
        .await
        .expect("Failed to insert test function");
    }

    #[tokio::test]
    async fn test_record_and_retrieve_invocation() {
        let pool = setup_test_db().await;
        let collector = MetricsCollector::new(pool.clone()).await;

        let function_id = FunctionId::new();
        insert_test_function(&pool, function_id, "test-function").await;

        let record = InvocationRecord {
            invocation_id: InvocationId::new(),
            function_id,
            function_name: "test-function".to_string(),
            version: None,
            invocation_type: InvocationType::RequestResponse,
            cold_start: true,
            duration_ms: Some(100),
            memory_used_bytes: Some(1024 * 1024),
            cpu_time_ms: Some(50),
            success: true,
            error_message: None,
            error_type: None,
            started_at: 1000,
            completed_at: Some(1100),
        };

        collector
            .record_invocation(record.clone())
            .await
            .expect("Failed to record invocation");

        let recent = collector
            .get_recent_invocations(&function_id, 10)
            .await
            .expect("Failed to get recent invocations");

        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].invocation_id, record.invocation_id);
        assert_eq!(recent[0].duration_ms, Some(100));
    }

    #[tokio::test]
    async fn test_function_metrics() {
        let pool = setup_test_db().await;
        let collector = MetricsCollector::new(pool.clone()).await;

        let function_id = FunctionId::new();
        insert_test_function(&pool, function_id, "test-function").await;

        // Insert multiple invocations
        for i in 0..10 {
            let record = InvocationRecord {
                invocation_id: InvocationId::new(),
                function_id,
                function_name: "test-function".to_string(),
                version: None,
                invocation_type: InvocationType::RequestResponse,
                cold_start: i == 0, // Only first is cold start
                duration_ms: Some(100 + i * 10),
                memory_used_bytes: Some(1024 * 1024),
                cpu_time_ms: Some(50),
                success: i % 3 != 0, // Fail every 3rd invocation
                error_message: if i % 3 == 0 {
                    Some("Test error".to_string())
                } else {
                    None
                },
                error_type: if i % 3 == 0 {
                    Some("TestError".to_string())
                } else {
                    None
                },
                started_at: 1000 + i,
                completed_at: Some(1100 + i),
            };

            collector
                .record_invocation(record)
                .await
                .expect("Failed to record invocation");
        }

        let metrics = collector
            .get_function_metrics(&function_id, None, None)
            .await
            .expect("Failed to get metrics");

        assert_eq!(metrics.total_invocations, 10);
        assert_eq!(metrics.cold_starts, 1);
        assert_eq!(metrics.successful_invocations, 7);
        assert_eq!(metrics.failed_invocations, 3);
        assert!(metrics.avg_duration_ms > 0.0);
        assert!(metrics.p50_duration_ms > 0);
        assert!(metrics.p95_duration_ms > 0);
        assert!(metrics.p99_duration_ms > 0);
    }

    #[tokio::test]
    async fn test_global_stats() {
        let pool = setup_test_db().await;
        let collector = MetricsCollector::new(pool.clone()).await;

        let function_id1 = FunctionId::new();
        let function_id2 = FunctionId::new();
        insert_test_function(&pool, function_id1, "function-1").await;
        insert_test_function(&pool, function_id2, "function-2").await;

        // Insert invocations for both functions
        for func_id in [function_id1, function_id2] {
            for i in 0..5 {
                let record = InvocationRecord {
                    invocation_id: InvocationId::new(),
                    function_id: func_id,
                    function_name: format!("function-{}", if func_id == function_id1 { 1 } else { 2 }),
                    version: None,
                    invocation_type: InvocationType::RequestResponse,
                    cold_start: i == 0,
                    duration_ms: Some(100),
                    memory_used_bytes: Some(1024 * 1024),
                    cpu_time_ms: Some(50),
                    success: true,
                    error_message: None,
                    error_type: None,
                    started_at: 1000 + i,
                    completed_at: Some(1100 + i),
                };

                collector
                    .record_invocation(record)
                    .await
                    .expect("Failed to record invocation");
            }
        }

        let stats = collector
            .get_global_stats()
            .await
            .expect("Failed to get global stats");

        assert_eq!(stats.total_invocations, 10);
        assert_eq!(stats.total_functions, 2);
        assert_eq!(stats.cold_starts, 2);
        assert_eq!(stats.successful_invocations, 10);
        assert_eq!(stats.failed_invocations, 0);
    }

    #[tokio::test]
    async fn test_cleanup_old_records() {
        let pool = setup_test_db().await;
        let collector = MetricsCollector::new(pool.clone()).await;

        let function_id = FunctionId::new();
        insert_test_function(&pool, function_id, "test-function").await;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Insert old records
        for i in 0..5 {
            let record = InvocationRecord {
                invocation_id: InvocationId::new(),
                function_id,
                function_name: "test-function".to_string(),
                version: None,
                invocation_type: InvocationType::RequestResponse,
                cold_start: false,
                duration_ms: Some(100),
                memory_used_bytes: Some(1024 * 1024),
                cpu_time_ms: Some(50),
                success: true,
                error_message: None,
                error_type: None,
                started_at: now - 86400000 - i, // More than 1 day old
                completed_at: Some(now - 86400000 - i + 100),
            };

            collector
                .record_invocation(record)
                .await
                .expect("Failed to record invocation");
        }

        // Insert recent records
        for i in 0..3 {
            let record = InvocationRecord {
                invocation_id: InvocationId::new(),
                function_id,
                function_name: "test-function".to_string(),
                version: None,
                invocation_type: InvocationType::RequestResponse,
                cold_start: false,
                duration_ms: Some(100),
                memory_used_bytes: Some(1024 * 1024),
                cpu_time_ms: Some(50),
                success: true,
                error_message: None,
                error_type: None,
                started_at: now - 1000 - i, // Very recent
                completed_at: Some(now - 900 - i),
            };

            collector
                .record_invocation(record)
                .await
                .expect("Failed to record invocation");
        }

        // Clean up records older than 1 day
        let deleted = collector
            .cleanup_old_records(86400000)
            .await
            .expect("Failed to cleanup");

        assert_eq!(deleted, 5);

        let metrics = collector
            .get_function_metrics(&function_id, None, None)
            .await
            .expect("Failed to get metrics");

        assert_eq!(metrics.total_invocations, 3);
    }

    #[tokio::test]
    async fn test_metrics_with_time_range() {
        let pool = setup_test_db().await;
        let collector = MetricsCollector::new(pool.clone()).await;

        let function_id = FunctionId::new();
        insert_test_function(&pool, function_id, "test-function").await;

        // Insert invocations at different times
        for i in 0..10 {
            let record = InvocationRecord {
                invocation_id: InvocationId::new(),
                function_id,
                function_name: "test-function".to_string(),
                version: None,
                invocation_type: InvocationType::RequestResponse,
                cold_start: false,
                duration_ms: Some(100 + i * 10),
                memory_used_bytes: Some(1024 * 1024),
                cpu_time_ms: Some(50),
                success: true,
                error_message: None,
                error_type: None,
                started_at: 1000 + i * 1000, // 1 second apart
                completed_at: Some(1100 + i * 1000),
            };

            collector
                .record_invocation(record)
                .await
                .expect("Failed to record invocation");
        }

        // Get metrics for a specific time range (first 5 invocations)
        let metrics = collector
            .get_function_metrics(&function_id, Some(1000), Some(5000))
            .await
            .expect("Failed to get metrics");

        assert_eq!(metrics.total_invocations, 5);

        // Get metrics for all time
        let all_metrics = collector
            .get_function_metrics(&function_id, None, None)
            .await
            .expect("Failed to get metrics");

        assert_eq!(all_metrics.total_invocations, 10);
    }

    #[tokio::test]
    async fn test_no_invocations() {
        let pool = setup_test_db().await;
        let collector = MetricsCollector::new(pool.clone()).await;

        let function_id = FunctionId::new();
        insert_test_function(&pool, function_id, "test-function").await;

        let metrics = collector
            .get_function_metrics(&function_id, None, None)
            .await
            .expect("Failed to get metrics");

        assert_eq!(metrics.total_invocations, 0);
        assert_eq!(metrics.successful_invocations, 0);
        assert_eq!(metrics.failed_invocations, 0);
        assert_eq!(metrics.cold_starts, 0);
        assert_eq!(metrics.avg_duration_ms, 0.0);
        assert_eq!(metrics.p50_duration_ms, 0);
        assert_eq!(metrics.p95_duration_ms, 0);
        assert_eq!(metrics.p99_duration_ms, 0);
    }
}
