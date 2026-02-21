//! JsExec activation - JavaScript execution in sandboxed V8 isolates
//!
//! This module provides the main JsExec activation that integrates with
//! the Plexus hub system using plexus_macros.

use crate::runner::{self, RunnerConfig};
use crate::types::{
    ExecutionContext, JsExecConfig, JsExecEvent,
    ScriptId, ScriptMetadata,
};
use async_stream::stream;
use futures::Stream;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

// Re-export plexus_core types needed by plexus_macros generated code
#[allow(unused_imports)]
use plexus_core::plexus::{
    Activation, ChildSummary, MethodSchema, PlexusError, PlexusStream, PluginSchema, SchemaResult,
    wrap_stream,
};
#[allow(unused_imports)]
use plexus_core::serde_helpers;

// =============================================================================
// Script Storage
// =============================================================================

/// A stored script with its code and metadata
#[derive(Debug, Clone)]
pub struct StoredScript {
    /// The JavaScript source code
    pub code: String,
    /// Metadata about the script
    pub metadata: ScriptMetadata,
}

/// In-memory script store
#[derive(Debug)]
pub struct ScriptStore {
    scripts: RwLock<HashMap<ScriptId, StoredScript>>,
}

impl ScriptStore {
    pub fn new() -> Self {
        Self {
            scripts: RwLock::new(HashMap::new()),
        }
    }

    pub async fn store(&self, id: ScriptId, code: String, metadata: ScriptMetadata) {
        let stored = StoredScript { code, metadata };
        self.scripts.write().await.insert(id, stored);
    }

    pub async fn get(&self, id: &ScriptId) -> Option<StoredScript> {
        self.scripts.read().await.get(id).cloned()
    }

    pub async fn delete(&self, id: &ScriptId) -> bool {
        self.scripts.write().await.remove(id).is_some()
    }

    pub async fn list(&self) -> Vec<ScriptMetadata> {
        self.scripts
            .read()
            .await
            .values()
            .map(|s| s.metadata.clone())
            .collect()
    }
}

impl Default for ScriptStore {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// JsExec Activation
// =============================================================================

/// JsExec activation - execute JavaScript in ephemeral V8 isolates
#[derive(Clone)]
pub struct JsExec {
    scripts: Arc<ScriptStore>,
    config: JsExecConfig,
    runner_config: RunnerConfig,
}

impl JsExec {
    /// Create a new JsExec instance with the given configuration
    pub fn new(config: JsExecConfig) -> Self {
        // Copy modules and node_modules_path from config to runner_config
        let runner_config = RunnerConfig {
            modules: config.modules.clone(),
            node_modules_path: config.node_modules_path.clone(),
            ..RunnerConfig::default()
        };
        Self {
            scripts: Arc::new(ScriptStore::new()),
            config,
            runner_config,
        }
    }

    /// Compute MD5 hash of code content
    fn hash_code(code: &str) -> String {
        let mut hasher = Md5::new();
        hasher.update(code.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Get current timestamp in milliseconds
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Bundle npm packages and return them as inline modules
    async fn bundle_npm_packages(&self) -> Result<Vec<crate::types::ModuleConfig>, String> {
        use crate::bundler::{bundle_package, BundleConfig, BundleFormat};

        if self.config.npm_packages.is_empty() {
            return Ok(vec![]);
        }

        // Need node_modules path to bundle
        let node_modules_path = self.config.node_modules_path.as_ref()
            .ok_or_else(|| "npm_packages specified but node_modules_path not configured".to_string())?;

        let bundle_config = BundleConfig {
            esbuild_path: self.config.esbuild_path.clone()
                .unwrap_or_else(|| std::path::PathBuf::from("esbuild")),
            node_modules_path: node_modules_path.clone(),
            format: BundleFormat::Esm,
        };

        let mut bundled_modules = Vec::new();

        for package_name in &self.config.npm_packages {
            // Bundle the package (TODO: add caching)
            let bundled_code = bundle_package(package_name, &bundle_config)
                .await
                .map_err(|e| format!("Failed to bundle {}: {}", package_name, e))?;

            // Create an inline module with the bundled code
            bundled_modules.push(crate::types::ModuleConfig {
                name: package_name.clone(),
                source: crate::types::ModuleSource::Code(bundled_code),
                expose_global: false,
            });
        }

        Ok(bundled_modules)
    }

    /// Bundle local libraries (TypeScript/JavaScript) and return them as inline modules
    async fn bundle_local_libraries(&self) -> Result<Vec<crate::types::ModuleConfig>, String> {
        use crate::bundler::{bundle_local_library, BundleConfig, BundleFormat, NoCache};

        if self.config.local_libraries.is_empty() {
            return Ok(vec![]);
        }

        let bundle_config = BundleConfig {
            esbuild_path: self.config.esbuild_path.clone()
                .unwrap_or_else(|| std::path::PathBuf::from("esbuild")),
            node_modules_path: std::path::PathBuf::from("."),  // Not used for local libs
            format: BundleFormat::Esm,
        };

        let tsc_path = self.config.tsc_path.as_ref().map(|p| p.as_path());

        let mut bundled_modules = Vec::new();

        for library in &self.config.local_libraries {
            // Bundle with type checking and caching
            // TODO: Use a real cache instead of NoCache
            let no_cache = NoCache;
            let bundled_code = bundle_local_library(
                library,
                &bundle_config,
                Some(&no_cache),
                tsc_path,
            )
            .await
            .map_err(|e| format!("Failed to bundle local library '{}': {}", library.name, e))?;

            // Create an inline module with the bundled code
            bundled_modules.push(crate::types::ModuleConfig {
                name: library.name.clone(),
                source: crate::types::ModuleSource::Code(bundled_code),
                expose_global: false,
            });
        }

        Ok(bundled_modules)
    }

    /// Bundle all configured libraries (npm packages + local libraries)
    async fn bundle_all_libraries(&self) -> Result<Vec<crate::types::ModuleConfig>, String> {
        let mut all_bundled = Vec::new();

        // Bundle npm packages
        let npm_modules = self.bundle_npm_packages().await?;
        all_bundled.extend(npm_modules);

        // Bundle local libraries
        let local_modules = self.bundle_local_libraries().await?;
        all_bundled.extend(local_modules);

        Ok(all_bundled)
    }
}

/// Plexus-macros generates all the boilerplate for this impl block
#[plexus_macros::hub_methods(
    namespace = "jsexec",
    version = "1.0.0",
    description = "Execute JavaScript in sandboxed V8 isolates",
    crate_path = "plexus_core"
)]
impl JsExec {
    /// Execute JavaScript code and stream results
    #[plexus_macros::hub_method(
        description = "Execute JavaScript code with streaming output",
        params(code = "JavaScript source code to execute")
    )]
    async fn execute(
        &self,
        code: String,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let self_clone = self.clone();

        stream! {
            // Bundle all configured libraries (npm + local)
            let bundled_modules = match self_clone.bundle_all_libraries().await {
                Ok(modules) => modules,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle libraries: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Merge bundled modules with preloaded modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.extend(bundled_modules);

            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                ..self_clone.runner_config.clone()
            };

            // Execute with all modules available
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// Execute JavaScript code with additional modules loaded by path
    #[plexus_macros::hub_method(
        description = "Execute JavaScript code with additional modules loaded from file paths",
        params(
            code = "JavaScript source code to execute",
            module_paths = "Array of file paths to load as modules. Module names are derived from filenames."
        )
    )]
    async fn execute_with_modules(
        &self,
        code: String,
        module_paths: Vec<String>,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let self_clone = self.clone();

        stream! {
            // Bundle all configured libraries (npm + local)
            let bundled_modules = match self_clone.bundle_all_libraries().await {
                Ok(modules) => modules,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle libraries: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Build runner config with additional modules from paths
            let mut additional_modules: Vec<crate::types::ModuleConfig> = module_paths
                .into_iter()
                .map(|path| crate::types::ModuleConfig::from_path(path))
                .collect();

            // Combine preloaded + bundled + path-based modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.extend(bundled_modules);
            all_modules.append(&mut additional_modules);

            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                ..self_clone.runner_config.clone()
            };

            // Execute with all modules available
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// Evaluate a JavaScript expression and return the result
    #[plexus_macros::hub_method(
        description = "Evaluate a JavaScript expression and return the result",
        params(expr = "JavaScript expression to evaluate")
    )]
    async fn eval(&self, expr: String) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let code = format!("return ({})", expr);
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let self_clone = self.clone();

        stream! {
            // Bundle all configured libraries (npm + local)
            let bundled_modules = match self_clone.bundle_all_libraries().await {
                Ok(modules) => modules,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle libraries: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Merge bundled modules with preloaded modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.extend(bundled_modules);

            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                ..self_clone.runner_config.clone()
            };

            // Execute with all modules available
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// Store a script for later execution
    #[plexus_macros::hub_method(
        description = "Store a script for later execution",
        params(
            name = "Human-readable name for the script",
            code = "JavaScript source code",
            description = "Optional description of what the script does"
        )
    )]
    async fn store(
        &self,
        name: String,
        code: String,
        description: Option<String>,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let scripts = self.scripts.clone();
        let code_clone = code.clone();
        let name_clone = name.clone();

        stream! {
            let id = ScriptId::new();
            let now = Self::now_ms();
            let size_bytes = code_clone.len() as u64;
            let hash = Self::hash_code(&code_clone);

            let metadata = ScriptMetadata {
                id,
                name: name_clone.clone(),
                size_bytes,
                hash: hash.clone(),
                created_at: now,
                updated_at: now,
                description,
                tags: vec![],
            };

            scripts.store(id, code_clone, metadata).await;

            yield JsExecEvent::ScriptStored {
                script_id: id,
                name: name_clone,
                size_bytes,
                hash,
            };
        }
    }

    /// Run a previously stored script
    #[plexus_macros::hub_method(
        description = "Run a previously stored script by ID",
        params(script_id = "ID of the stored script to run")
    )]
    async fn execute_script(
        &self,
        script_id: ScriptId,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let scripts = self.scripts.clone();
        let runner_config = self.runner_config.clone();
        let limits = self.config.default_limits.clone();

        stream! {
            // Load the script from storage
            let stored = match scripts.get(&script_id).await {
                Some(s) => s,
                None => {
                    yield JsExecEvent::Error {
                        message: format!("Script not found: {}", script_id),
                        name: "ScriptNotFound".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Execute the stored script
            let context = ExecutionContext::default();
            let mut result_stream = std::pin::pin!(
                runner::execute(stored.code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// List all stored scripts
    #[plexus_macros::hub_method(description = "List all stored scripts")]
    async fn list_scripts(&self) -> impl Stream<Item = ScriptMetadata> + Send + 'static {
        let scripts = self.scripts.clone();

        stream! {
            let all_scripts = scripts.list().await;
            for metadata in all_scripts {
                yield metadata;
            }
        }
    }

    /// Delete a stored script
    #[plexus_macros::hub_method(
        description = "Delete a stored script by ID",
        params(script_id = "ID of the script to delete")
    )]
    async fn delete_script(
        &self,
        script_id: ScriptId,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let scripts = self.scripts.clone();

        stream! {
            if scripts.delete(&script_id).await {
                yield JsExecEvent::ScriptDeleted { script_id };
            } else {
                yield JsExecEvent::Error {
                    message: format!("Script not found: {}", script_id),
                    name: "ScriptNotFound".to_string(),
                    location: None,
                    stack: vec![],
                };
            }
        }
    }

    /// Execute JavaScript with a typed Plexus client for a backend
    #[plexus_macros::hub_method(
        description = "Execute JavaScript with a typed Plexus client for a backend",
        params(
            host = "Plexus backend host",
            port = "Plexus backend port",
            backend = "Backend name (e.g. 'substrate')",
            code = "JavaScript code to execute"
        )
    )]
    async fn plexus_env(
        &self,
        host: String,
        port: u16,
        backend: String,
        code: String,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let self_clone = self.clone();

        stream! {
            // Stage 1: Discover tools
            yield JsExecEvent::PlexusEnvProgress {
                stage: "discovering_tools".to_string(),
                message: "Discovering synapse, hub-codegen, and esbuild binaries".to_string(),
            };

            let tools = match crate::plexus_env::discover_tools(&self_clone.config).await {
                Ok(t) => t,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Tool discovery failed: {}", e),
                        name: "PlexusEnvError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Stage 2: Generate IR
            yield JsExecEvent::PlexusEnvProgress {
                stage: "generating_ir".to_string(),
                message: format!("Generating IR for backend '{}' at {}:{}", backend, host, port),
            };

            let (ir_json, ir_hash) = match crate::plexus_env::generate_ir(
                &tools.synapse, &host, port, &backend,
            ).await {
                Ok(r) => r,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("IR generation failed: {}", e),
                        name: "PlexusEnvError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Stage 3: Check cache
            let cache_dir = self_clone.config.plexus_cache_dir.clone()
                .unwrap_or_else(crate::plexus_env::PlexusClientCache::default_dir);
            let cache = crate::plexus_env::PlexusClientCache::new(cache_dir);

            let bundle = if let Some(cached) = cache.get(&backend, &ir_hash) {
                yield JsExecEvent::PlexusEnvProgress {
                    stage: "cache_hit".to_string(),
                    message: format!("Using cached bundle for {}_{}", backend, ir_hash),
                };
                cached
            } else {
                // Stage 4: Generate TypeScript
                yield JsExecEvent::PlexusEnvProgress {
                    stage: "generating_code".to_string(),
                    message: "Running hub-codegen to generate TypeScript client".to_string(),
                };

                let temp_dir = match tempfile::tempdir() {
                    Ok(d) => d,
                    Err(e) => {
                        yield JsExecEvent::Error {
                            message: format!("Failed to create temp dir: {}", e),
                            name: "PlexusEnvError".to_string(),
                            location: None,
                            stack: vec![],
                        };
                        return;
                    }
                };

                if let Err(e) = crate::plexus_env::generate_typescript(
                    &tools.hub_codegen, &ir_json, temp_dir.path(),
                ).await {
                    yield JsExecEvent::Error {
                        message: format!("Code generation failed: {}", e),
                        name: "PlexusEnvError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }

                // Stage 5: Bundle
                yield JsExecEvent::PlexusEnvProgress {
                    stage: "bundling".to_string(),
                    message: "Bundling TypeScript client with esbuild".to_string(),
                };

                let bundled = match crate::plexus_env::bundle_to_esm(
                    &tools.esbuild, temp_dir.path(),
                ).await {
                    Ok(b) => b,
                    Err(e) => {
                        yield JsExecEvent::Error {
                            message: format!("Bundling failed: {}", e),
                            name: "PlexusEnvError".to_string(),
                            location: None,
                            stack: vec![],
                        };
                        return;
                    }
                };

                // Cache the result
                if let Err(e) = cache.put(&backend, &ir_hash, &bundled) {
                    tracing::warn!("Failed to cache plexus client bundle: {}", e);
                }

                bundled
            };

            // Build modules: ws shim + plexus client
            let ws_module = crate::types::ModuleConfig::inline(
                "ws",
                crate::plexus_env::WS_SHIM,
            );
            let plexus_module = crate::types::ModuleConfig::inline(
                "plexus",
                &bundle,
            );

            // Merge with existing configured modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.push(ws_module);
            all_modules.push(plexus_module);

            // Build execution context with plexus_url
            let plexus_url = format!("ws://{}:{}", host, port);
            let mut context = ExecutionContext::default();
            context.data = serde_json::json!({
                "plexus_url": plexus_url,
            });

            let limits = self_clone.config.default_limits.clone();
            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                force_nodejs_compat: true,
                ..self_clone.runner_config.clone()
            };

            // Execute the user code
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }
}

// =============================================================================
// Public API for direct use
// =============================================================================

impl JsExec {
    /// Run JavaScript code and stream execution events (public API)
    pub fn run(&self, code: String) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let self_clone = self.clone();

        stream! {
            // Bundle all configured libraries (npm + local)
            let bundled_modules = match self_clone.bundle_all_libraries().await {
                Ok(modules) => modules,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle libraries: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Merge bundled modules with preloaded modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.extend(bundled_modules);

            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                ..self_clone.runner_config.clone()
            };

            // Execute with all modules available
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// Evaluate a JavaScript expression (public API)
    pub fn evaluate(&self, expr: String) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let code = format!("return ({})", expr);
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let self_clone = self.clone();

        stream! {
            // Bundle all configured libraries (npm + local)
            let bundled_modules = match self_clone.bundle_all_libraries().await {
                Ok(modules) => modules,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle libraries: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Merge bundled modules with preloaded modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.extend(bundled_modules);

            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                ..self_clone.runner_config.clone()
            };

            // Execute with all modules available
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// Run JavaScript code with additional modules loaded from file paths (public API)
    /// Module names are derived from the filenames (without extension)
    pub fn run_with_modules(
        &self,
        code: String,
        module_paths: Vec<String>,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let self_clone = self.clone();

        stream! {
            // Bundle all configured libraries (npm + local)
            let bundled_modules = match self_clone.bundle_all_libraries().await {
                Ok(modules) => modules,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle libraries: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Build runner config with additional modules from paths
            let mut additional_modules: Vec<crate::types::ModuleConfig> = module_paths
                .into_iter()
                .map(|path| crate::types::ModuleConfig::from_path(path))
                .collect();

            // Combine preloaded + bundled + path-based modules
            let mut all_modules = self_clone.runner_config.modules.clone();
            all_modules.extend(bundled_modules);
            all_modules.append(&mut additional_modules);

            let runner_config = runner::RunnerConfig {
                modules: all_modules,
                node_modules_path: self_clone.runner_config.node_modules_path.clone(),
                ..self_clone.runner_config.clone()
            };

            // Execute with all modules available
            let mut result_stream = std::pin::pin!(
                runner::execute(code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }

    /// Get all stored scripts (public API)
    pub async fn get_scripts(&self) -> Vec<ScriptMetadata> {
        self.scripts.list().await
    }

    /// Run a JavaScript/TypeScript file with automatic dependency bundling
    ///
    /// This method automatically:
    /// - Bundles the file with esbuild (resolving all imports)
    /// - Compiles TypeScript if needed
    /// - Caches the bundled result (keyed by file content hash)
    /// - Executes the bundled code
    ///
    /// # Example
    /// ```no_run
    /// # use plexus_jsexec::{JsExec, JsExecConfig};
    /// # let jsexec = JsExec::new(JsExecConfig::default());
    /// // Just run the file - all dependencies auto-bundled
    /// jsexec.run_file("./src/my-script.ts");
    /// ```
    pub fn run_file(
        &self,
        path: impl AsRef<std::path::Path>
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        self.run_file_with_typecheck(path, false)
    }

    /// Run a file with type checking enabled
    pub fn run_file_checked(
        &self,
        path: impl AsRef<std::path::Path>
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        self.run_file_with_typecheck(path, true)
    }

    /// Internal implementation for run_file with optional type checking
    fn run_file_with_typecheck(
        &self,
        path: impl AsRef<std::path::Path>,
        typecheck: bool,
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static {
        use crate::bundler::{bundle_local_library, BundleConfig, BundleFormat, NoCache};

        let path = path.as_ref().to_path_buf();
        let context = ExecutionContext::default();
        let limits = self.config.default_limits.clone();
        let runner_config = self.runner_config.clone();

        // Get esbuild and tsc paths
        let esbuild_path = self.config.esbuild_path.clone()
            .unwrap_or_else(|| std::path::PathBuf::from("esbuild"));
        let tsc_path = self.config.tsc_path.clone().map(|p| p.to_path_buf());

        stream! {
            // Read the file to get content for bundling
            if !path.exists() {
                yield JsExecEvent::Error {
                    message: format!("File not found: {}", path.display()),
                    name: "FileNotFound".to_string(),
                    location: None,
                    stack: vec![],
                };
                return;
            }

            // Read the file content
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to read file: {}", e),
                        name: "IoError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // Type check if requested
            if typecheck {
                if let Some(ref tsc) = tsc_path {
                    use crate::bundler::typecheck_file;
                    if let Err(e) = typecheck_file(&path, tsc).await {
                        yield JsExecEvent::Error {
                            message: format!("Type check failed: {}", e),
                            name: "TypeCheckError".to_string(),
                            location: None,
                            stack: vec![],
                        };
                        return;
                    }
                }
            }

            // Extract imports and wrap the rest in an async IIFE
            // This allows top-level returns while keeping imports at top level
            let (imports, code_without_imports) = extract_imports(&content);
            let wrapped_content = if imports.is_empty() {
                format!("export default await (async () => {{\n{}\n}})();", content)
            } else {
                format!(
                    "{}\n\nexport default await (async () => {{\n{}\n}})();",
                    imports,
                    code_without_imports
                )
            };

            // Determine node_modules path (try multiple strategies)
            let node_modules_path = path.parent()
                .and_then(|p| {
                    let nm = p.join("node_modules");
                    if nm.exists() { Some(nm) } else { None }
                })
                .or_else(|| {
                    // Try parent's parent (for substrate-sandbox-ts structure)
                    path.parent().and_then(|p| p.parent()).and_then(|p| {
                        let nm = p.join("node_modules");
                        if nm.exists() { Some(nm) } else { None }
                    })
                })
                .or_else(|| {
                    // Try esbuild's directory (e.g., substrate-sandbox-ts/node_modules/.bin/esbuild)
                    esbuild_path.parent().and_then(|bin| {
                        bin.parent().map(|nm| nm.to_path_buf())
                    }).filter(|nm| nm.exists() && nm.file_name().and_then(|n| n.to_str()) == Some("node_modules"))
                })
                .or_else(|| {
                    // Try runner_config node_modules_path
                    runner_config.node_modules_path.clone().filter(|nm| nm.exists())
                })
                .unwrap_or_else(|| std::path::PathBuf::from("node_modules"));

            // Create temp file in node_modules parent directory so esbuild can resolve modules
            let base_dir = node_modules_path.parent().unwrap_or(std::path::Path::new("."));
            let temp_dir = match tempfile::tempdir_in(base_dir) {
                Ok(dir) => dir,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to create temp directory: {}", e),
                        name: "IoError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            let temp_file = temp_dir.path().join("input.ts");
            if let Err(e) = tokio::fs::write(&temp_file, &wrapped_content).await {
                yield JsExecEvent::Error {
                    message: format!("Failed to write temp file: {}", e),
                    name: "IoError".to_string(),
                    location: None,
                    stack: vec![],
                };
                return;
            }

            // Create a LocalLibrary config for the wrapped file
            let library = crate::types::LocalLibrary {
                name: "main".to_string(),
                entry_point: temp_file.clone(),
                typecheck: false,  // Already type-checked above if requested
            };

            // Bundle configuration
            let bundle_config = BundleConfig {
                esbuild_path: esbuild_path.clone(),
                node_modules_path: node_modules_path.clone(),
                format: BundleFormat::Esm,
            };

            // Bundle the wrapped file (this handles all imports automatically)
            let no_cache = NoCache;  // TODO: Use real cache
            let bundled_code = match bundle_local_library(
                &library,
                &bundle_config,
                Some(&no_cache),
                None,  // No type checking needed, already done
            ).await {
                Ok(code) => code,
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to bundle file: {}", e),
                        name: "BundleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            };

            // The bundled code exports the result of the async IIFE
            // We need to import it and return the default export
            let execution_code = format!(
                "import result from 'main';\nreturn result;"
            );

            // Setup runner config with the bundled module and node_modules
            let mut runner_config = runner_config.clone();
            runner_config.modules.push(crate::types::ModuleConfig {
                name: "main".to_string(),
                source: crate::types::ModuleSource::Code(bundled_code),
                expose_global: false,
            });
            runner_config.node_modules_path = Some(node_modules_path);

            // Execute the import + return code
            let mut result_stream = std::pin::pin!(
                runner::execute(execution_code, context, limits, runner_config)
            );

            use futures::StreamExt;
            while let Some(event) = result_stream.next().await {
                yield event;
            }
        }
    }
}

/// Extract import statements from code and return (imports, code_without_imports)
/// This allows keeping imports at top level while wrapping the rest in a function
fn extract_imports(code: &str) -> (String, String) {
    use regex::Regex;

    let import_re = Regex::new(r#"(?m)^\s*import\s+.+?from\s+['"].+?['"]\s*;?\s*$"#).unwrap();

    let mut imports = Vec::new();
    let mut remaining_lines = Vec::new();

    for line in code.lines() {
        if import_re.is_match(line) {
            imports.push(line.trim().to_string());
        } else {
            remaining_lines.push(line.to_string());
        }
    }

    (
        imports.join("\n"),
        remaining_lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_store_new() {
        let store = ScriptStore::new();
        assert!(store.scripts.try_read().is_ok());
    }

    #[test]
    fn test_hash_code() {
        let hash1 = JsExec::hash_code("console.log('hello')");
        let hash2 = JsExec::hash_code("console.log('hello')");
        let hash3 = JsExec::hash_code("console.log('world')");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
