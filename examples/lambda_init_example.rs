//! Example of initializing the Lambda system
//!
//! This example demonstrates how to initialize the complete Lambda system
//! with all components (registry, runtime, invoker, metrics, etc.)

use plexus_jsexec::lambda::{LambdaConfig, LambdaSystem};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    // Create configuration
    let config = LambdaConfig::new("sqlite:lambda.db")
        .with_timeout(60000) // 60 seconds
        .with_memory(256);   // 256 MB

    // Initialize the Lambda system
    println!("Initializing Lambda system...");
    let system = LambdaSystem::initialize(config).await?;

    println!("Lambda system initialized successfully!");
    println!("Database URL: {}", system.config.database_url);
    println!("Metrics enabled: {}", system.metrics.is_some());
    println!("Default timeout: {} ms", system.config.default_timeout_ms);
    println!("Default memory: {} MB", system.config.default_memory_mb);

    // Verify database connection
    let pool = system.pool();
    println!("Database connection active: {}", !pool.is_closed());

    // Query the functions table to verify it exists
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM functions")
        .fetch_one(pool)
        .await?;
    println!("Functions in database: {}", count.0);

    println!("\nLambda system is ready to use!");

    Ok(())
}
