//! Plexus environment pipeline for jsexec
//!
//! Generates typed TypeScript clients for Plexus backends, bundles them,
//! and loads them into the workerd sandbox.
//!
//! Pipeline: synapse → IR JSON → hub-codegen → TypeScript → esbuild → single .js → workerd module

use std::path::{Path, PathBuf};

use crate::types::JsExecConfig;

/// Discovered tool binary paths
#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub synapse: PathBuf,
    pub hub_codegen: PathBuf,
    pub esbuild: PathBuf,
}

/// Whether the bundle came from cache or was freshly generated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleSource {
    CacheHit,
    Generated,
}

/// File-based cache for generated Plexus client bundles, keyed by `{backend}_{ir_hash}`
pub struct PlexusClientCache {
    cache_dir: PathBuf,
}

impl PlexusClientCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    pub fn default_dir() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("jsexec")
            .join("plexus")
    }

    fn cache_path(&self, backend: &str, hash: &str) -> PathBuf {
        self.cache_dir.join(format!("{}_{}.js", backend, hash))
    }

    pub fn get(&self, backend: &str, hash: &str) -> Option<String> {
        let path = self.cache_path(backend, hash);
        std::fs::read_to_string(path).ok()
    }

    pub fn put(&self, backend: &str, hash: &str, content: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::write(self.cache_path(backend, hash), content)
    }
}

/// Discover tool binaries: explicit config paths → well-known dirs → PATH
pub async fn discover_tools(config: &JsExecConfig) -> Result<ToolPaths, String> {
    let synapse = find_binary(
        "synapse",
        config.synapse_path.as_deref(),
        &["~/.cabal/bin"],
    )?;
    let hub_codegen = find_binary(
        "hub-codegen",
        config.hub_codegen_path.as_deref(),
        &["~/.cargo/bin", "~/.plexus/bin"],
    )?;
    let esbuild = find_binary(
        "esbuild",
        config.esbuild_path.as_deref(),
        &["~/.cargo/bin", "~/.plexus/bin"],
    )?;

    Ok(ToolPaths {
        synapse,
        hub_codegen,
        esbuild,
    })
}

/// Find a binary: explicit path → well-known directories → PATH via `which`
fn find_binary(
    name: &str,
    explicit: Option<&Path>,
    well_known_dirs: &[&str],
) -> Result<PathBuf, String> {
    // 1. Explicit config path
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(format!(
            "Configured path for {} does not exist: {}",
            name,
            p.display()
        ));
    }

    // 2. Well-known directories
    let home = dirs::home_dir();
    for dir in well_known_dirs {
        let expanded = if dir.starts_with("~/") {
            match &home {
                Some(h) => h.join(&dir[2..]),
                None => continue,
            }
        } else {
            PathBuf::from(dir)
        };
        let candidate = expanded.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // 3. PATH via `which`
    which::which(name).map_err(|_| format!("Could not find '{}' binary. Searched: configured paths, {:?}, and PATH", name, well_known_dirs))
}

/// Run synapse to get schema IR JSON and extract irHash for cache key
pub async fn generate_ir(
    synapse: &Path,
    host: &str,
    port: u16,
    backend: &str,
) -> Result<(String, String), String> {
    let output = tokio::process::Command::new(synapse)
        .args(["-H", host, "-P", &port.to_string(), "-i", backend])
        .output()
        .await
        .map_err(|e| format!("Failed to run synapse: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("synapse IR generation failed: {}", stderr));
    }

    let ir_json = String::from_utf8(output.stdout)
        .map_err(|e| format!("synapse output is not valid UTF-8: {}", e))?;

    // Extract irHash from the JSON
    let parsed: serde_json::Value = serde_json::from_str(&ir_json)
        .map_err(|e| format!("Failed to parse synapse IR JSON: {}", e))?;

    let ir_hash = parsed
        .get("irHash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "synapse IR JSON missing 'irHash' field".to_string())?
        .to_string();

    Ok((ir_json, ir_hash))
}

/// Run hub-codegen to generate TypeScript from IR
pub async fn generate_typescript(
    hub_codegen: &Path,
    ir_json: &str,
    output_dir: &Path,
) -> Result<(), String> {
    // Write IR to a temp file
    let ir_file = output_dir.join("ir.json");
    tokio::fs::write(&ir_file, ir_json)
        .await
        .map_err(|e| format!("Failed to write IR file: {}", e))?;

    let output = tokio::process::Command::new(hub_codegen)
        .args([
            "--target",
            "typescript",
            "--output",
            &output_dir.to_string_lossy(),
            "--bundle-transport=true",
        ])
        .arg(&ir_file)
        .output()
        .await
        .map_err(|e| format!("Failed to run hub-codegen: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("hub-codegen failed: {}", stderr));
    }

    Ok(())
}

/// Bundle generated TypeScript into a single ES module using esbuild
pub async fn bundle_to_esm(esbuild: &Path, generated_dir: &Path) -> Result<String, String> {
    let entry = generated_dir.join("index.ts");
    if !entry.exists() {
        return Err(format!(
            "Expected entry point not found: {}",
            entry.display()
        ));
    }

    let output = tokio::process::Command::new(esbuild)
        .args([
            &entry.to_string_lossy().to_string(),
            "--bundle",
            "--format=esm",
            "--platform=neutral",
            "--external:ws",
            "--minify",
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run esbuild: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("esbuild bundling failed: {}", stderr));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| format!("esbuild output is not valid UTF-8: {}", e))
}

/// Full pipeline: discover tools → synapse IR → hub-codegen → esbuild → cached bundle
pub async fn get_plexus_client_bundle(
    host: &str,
    port: u16,
    backend: &str,
    config: &JsExecConfig,
) -> Result<(String, BundleSource), String> {
    // 1. Discover tools
    let tools = discover_tools(config).await?;

    // 2. Generate IR and get hash
    let (ir_json, ir_hash) = generate_ir(&tools.synapse, host, port, backend).await?;

    // 3. Check cache
    let cache_dir = config
        .plexus_cache_dir
        .clone()
        .unwrap_or_else(PlexusClientCache::default_dir);
    let cache = PlexusClientCache::new(cache_dir);

    if let Some(cached) = cache.get(backend, &ir_hash) {
        return Ok((cached, BundleSource::CacheHit));
    }

    // 4. Generate TypeScript
    let temp_dir = tempfile::tempdir()
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;
    generate_typescript(&tools.hub_codegen, &ir_json, temp_dir.path()).await?;

    // 5. Bundle to ESM
    let bundle = bundle_to_esm(&tools.esbuild, temp_dir.path()).await?;

    // 6. Cache the result
    if let Err(e) = cache.put(backend, &ir_hash, &bundle) {
        tracing::warn!("Failed to cache plexus client bundle: {}", e);
    }

    Ok((bundle, BundleSource::Generated))
}

/// The ws shim module that re-exports globalThis.WebSocket as the `ws` package
pub const WS_SHIM: &str = r#"const WS = globalThis.WebSocket;
export default WS;
export { WS as WebSocket };
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_miss_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PlexusClientCache::new(dir.path().to_path_buf());
        assert!(cache.get("substrate", "abc123").is_none());
    }

    #[test]
    fn test_cache_put_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PlexusClientCache::new(dir.path().to_path_buf());
        cache.put("substrate", "abc123", "// bundled js").unwrap();
        let result = cache.get("substrate", "abc123");
        assert_eq!(result, Some("// bundled js".to_string()));
    }

    #[test]
    fn test_cache_different_hash_misses() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PlexusClientCache::new(dir.path().to_path_buf());
        cache.put("substrate", "abc123", "// v1").unwrap();
        assert!(cache.get("substrate", "def456").is_none());
    }

    #[test]
    fn test_cache_different_backend_misses() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PlexusClientCache::new(dir.path().to_path_buf());
        cache.put("substrate", "abc123", "// v1").unwrap();
        assert!(cache.get("other", "abc123").is_none());
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = PlexusClientCache::default_dir();
        assert!(dir.to_string_lossy().contains("jsexec"));
        assert!(dir.to_string_lossy().contains("plexus"));
    }

    #[test]
    fn test_ws_shim_content() {
        assert!(WS_SHIM.contains("globalThis.WebSocket"));
        assert!(WS_SHIM.contains("export default"));
        assert!(WS_SHIM.contains("export {"));
    }
}
