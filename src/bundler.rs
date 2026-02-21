//! NPM package bundler using esbuild
//!
//! This module handles auto-bundling of npm packages for use in workerd.
//! Since workerd doesn't have automatic node_modules resolution, we bundle
//! packages on-demand into single ES modules.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use async_trait::async_trait;

/// Cache for bundled npm packages
///
/// This trait abstracts the storage mechanism for bundled packages.
/// Implementations can be:
/// - In-memory (HashMap)
/// - On-disk (filesystem cache)
/// - Distributed (Redis, S3, etc.)
///
/// Cache keys should include package name and version to handle updates.
#[async_trait]
pub trait BundleCache: Send + Sync {
    /// Get a bundled package from cache
    /// Returns None if not cached
    async fn get(&self, key: &str) -> Option<String>;

    /// Store a bundled package in cache
    async fn put(&self, key: &str, content: String);

    /// Generate a cache key for a package
    /// Default implementation uses package name, but can include version/hash
    fn cache_key(&self, package_name: &str, _node_modules_path: &Path) -> String {
        package_name.to_string()
    }
}

/// No-op cache that never stores anything
/// Used when caching is disabled
pub struct NoCache;

#[async_trait]
impl BundleCache for NoCache {
    async fn get(&self, _key: &str) -> Option<String> {
        None
    }

    async fn put(&self, _key: &str, _content: String) {
        // No-op
    }
}

/// Bundle configuration
#[derive(Debug, Clone)]
pub struct BundleConfig {
    /// Path to esbuild binary
    pub esbuild_path: PathBuf,
    /// Path to node_modules directory
    pub node_modules_path: PathBuf,
    /// Format for output (esm or cjs)
    pub format: BundleFormat,
}

#[derive(Debug, Clone, Copy)]
pub enum BundleFormat {
    Esm,
    Cjs,
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            esbuild_path: PathBuf::from("esbuild"),
            node_modules_path: PathBuf::from("node_modules"),
            format: BundleFormat::Esm,
        }
    }
}

/// Bundle an npm package using esbuild
///
/// Takes a package name (e.g., "typescript" or "@types/node") and bundles it
/// into a single ES module file that can be loaded by workerd.
///
/// # Example
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use plexus_jsexec::bundler::{bundle_package, BundleConfig};
///
/// let config = BundleConfig::default();
/// let bundled_code = bundle_package("lodash", &config).await?;
/// # Ok(())
/// # }
/// ```
pub async fn bundle_package(
    package_name: &str,
    config: &BundleConfig,
) -> Result<String, BundleError> {
    // Create entry point in the node_modules parent directory so esbuild can resolve modules
    let base_dir = config.node_modules_path.parent().unwrap_or(Path::new("."));
    let temp_dir = tempfile::tempdir_in(base_dir)
        .map_err(|e| BundleError::Io(format!("Failed to create temp dir: {}", e)))?;

    let entry_point = temp_dir.path().join("entry.js");
    let entry_content = format!(
        r#"export * from '{}';
import pkg from '{}';
export default pkg;
"#,
        package_name, package_name
    );

    tokio::fs::write(&entry_point, entry_content)
        .await
        .map_err(|e| BundleError::Io(format!("Failed to write entry point: {}", e)))?;

    // Run esbuild to bundle the package
    let format_flag = match config.format {
        BundleFormat::Esm => "esm",
        BundleFormat::Cjs => "cjs",
    };

    let output = Command::new(&config.esbuild_path)
        .arg(entry_point.to_str().unwrap())
        .arg("--bundle")
        .arg("--format=".to_owned() + format_flag)
        .arg("--platform=browser")
        .arg("--minify")  // Minify for smaller bundles
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| BundleError::EsbuildFailed(format!("Failed to spawn esbuild: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BundleError::EsbuildFailed(format!(
            "esbuild failed for package '{}': {}",
            package_name, stderr
        )));
    }

    let bundled_code = String::from_utf8(output.stdout)
        .map_err(|e| BundleError::Io(format!("Invalid UTF-8 in esbuild output: {}", e)))?;

    Ok(bundled_code)
}

/// Bundle an npm package with caching
///
/// First checks the cache, then bundles if not found, then stores in cache.
pub async fn bundle_package_cached<C: BundleCache>(
    package_name: &str,
    config: &BundleConfig,
    cache: &C,
) -> Result<String, BundleError> {
    let cache_key = cache.cache_key(package_name, &config.node_modules_path);

    // Try cache first
    if let Some(cached) = cache.get(&cache_key).await {
        return Ok(cached);
    }

    // Bundle the package
    let bundled = bundle_package(package_name, config).await?;

    // Store in cache
    cache.put(&cache_key, bundled.clone()).await;

    Ok(bundled)
}

/// Errors that can occur during bundling
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("IO error: {0}")]
    Io(String),

    #[error("esbuild failed: {0}")]
    EsbuildFailed(String),

    #[error("Package not found: {0}")]
    PackageNotFound(String),

    #[error("Type checking failed: {0}")]
    TypeCheckFailed(String),
}

// =============================================================================
// Local Library Bundling (with TypeScript support and caching)
// =============================================================================

/// Compute MD5 hash of file content for cache keys
fn compute_file_hash(path: &Path) -> Result<String, BundleError> {
    use md5::{Digest, Md5};

    let content = std::fs::read(path)
        .map_err(|e| BundleError::Io(format!("Failed to read {}: {}", path.display(), e)))?;

    let mut hasher = Md5::new();
    hasher.update(&content);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Type check a TypeScript file using tsc
pub async fn typecheck_file(
    file_path: &Path,
    tsc_path: &Path,
) -> Result<(), BundleError> {
    let output = Command::new(tsc_path)
        .arg("--noEmit")  // Just check, don't output files
        .arg("--skipLibCheck")  // Skip checking node_modules
        .arg(file_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| BundleError::TypeCheckFailed(format!("Failed to spawn tsc: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{}\n{}", stdout, stderr);
        return Err(BundleError::TypeCheckFailed(combined));
    }

    Ok(())
}

/// Bundle a local library (TypeScript or JavaScript) with optional type checking and caching
///
/// # Arguments
/// * `library` - The local library configuration
/// * `config` - Bundle configuration (esbuild path, etc.)
/// * `cache` - Optional cache for bundled output
/// * `tsc_path` - Path to tsc binary (for type checking)
///
/// # Caching Strategy
/// - Cache key is based on MD5 hash of the entry point file
/// - On cache hit: return cached bundle (skip typecheck and bundling)
/// - On cache miss: typecheck (if enabled), bundle, then cache
///
/// # Type Checking
/// - Only runs if `library.typecheck` is true
/// - Uses tsc with --noEmit (just validates, doesn't compile)
/// - Fails the build if type errors are found
pub async fn bundle_local_library<C: BundleCache>(
    library: &crate::types::LocalLibrary,
    config: &BundleConfig,
    cache: Option<&C>,
    tsc_path: Option<&Path>,
) -> Result<String, BundleError> {
    // Compute hash of entry point for cache key
    let content_hash = compute_file_hash(&library.entry_point)?;
    let cache_key = format!("local_lib_{}_{}", library.name, content_hash);

    // Try cache first
    if let Some(cache) = cache {
        if let Some(cached) = cache.get(&cache_key).await {
            return Ok(cached);
        }
    }

    // Type check if requested
    if library.typecheck {
        let tsc = tsc_path
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var("TSC_PATH").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("tsc"));

        typecheck_file(&library.entry_point, &tsc).await?;
    }

    // Bundle with esbuild (handles TypeScript automatically)
    let output = Command::new(&config.esbuild_path)
        .arg(library.entry_point.to_str().unwrap())
        .arg("--bundle")
        .arg("--format=esm")
        .arg("--platform=browser")
        .arg("--minify")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| BundleError::EsbuildFailed(format!("Failed to spawn esbuild: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BundleError::EsbuildFailed(format!(
            "esbuild failed for library '{}': {}",
            library.name, stderr
        )));
    }

    let bundled_code = String::from_utf8(output.stdout)
        .map_err(|e| BundleError::Io(format!("Invalid UTF-8 in esbuild output: {}", e)))?;

    // Cache the result
    if let Some(cache) = cache {
        cache.put(&cache_key, bundled_code.clone()).await;
    }

    Ok(bundled_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundle_config_default() {
        let config = BundleConfig::default();
        assert_eq!(config.esbuild_path, PathBuf::from("esbuild"));
    }

    #[test]
    fn test_no_cache() {
        let cache = NoCache;
        assert_eq!(cache.cache_key("lodash", Path::new("/tmp")), "lodash");
    }

    struct MockCache {
        data: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    }

    impl MockCache {
        fn new() -> Self {
            Self {
                data: std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl BundleCache for MockCache {
        async fn get(&self, key: &str) -> Option<String> {
            self.data.read().await.get(key).cloned()
        }

        async fn put(&self, key: &str, content: String) {
            self.data.write().await.insert(key.to_string(), content);
        }
    }

    #[tokio::test]
    async fn test_mock_cache() {
        let cache = MockCache::new();

        assert!(cache.get("test").await.is_none());

        cache.put("test", "content".to_string()).await;

        assert_eq!(cache.get("test").await, Some("content".to_string()));
    }
}
