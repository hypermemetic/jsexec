//! Ephemeral runner for JavaScript execution
//!
//! Each execution spawns a fresh workerd process with the user's code
//! embedded directly as the worker module. No eval required.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use async_stream::stream;
use futures_util::StreamExt;
use reqwest::Client;
use tokio::process::Command;
use tokio_stream::Stream;
use tracing::{debug, trace, warn};

use crate::types::{ExecutionContext, ExecutionLimits, JsExecEvent, LogLevel, ModuleConfig, ModuleSource, SourceLocation, StackFrame};

/// Global port counter for unique port allocation
static PORT_COUNTER: AtomicU16 = AtomicU16::new(30000);

/// Allocate a unique port for a worker
fn allocate_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Error type for runner operations
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Failed to create temp directory: {0}")]
    TempDir(#[from] std::io::Error),

    #[error("Failed to spawn workerd: {0}")]
    Spawn(String),

    #[error("Workerd failed to start within timeout")]
    StartupTimeout,

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Workerd process exited unexpectedly")]
    ProcessExited,
}

/// Configuration for the runner
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Path to the workerd binary
    pub workerd_binary: PathBuf,
    /// Timeout for workerd startup health check
    pub startup_timeout: Duration,
    /// Maximum attempts for health check
    pub health_check_attempts: u32,
    /// Delay between health check attempts
    pub health_check_delay: Duration,
    /// Preloaded modules available to all executions
    pub modules: Vec<ModuleConfig>,
    /// Path to node_modules directory for npm package resolution
    pub node_modules_path: Option<PathBuf>,
    /// Force nodejs_compat flag even without node_modules dir
    pub force_nodejs_compat: bool,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            workerd_binary: PathBuf::from("workerd"),
            startup_timeout: Duration::from_secs(5),
            health_check_attempts: 50,
            health_check_delay: Duration::from_millis(100),
            modules: Vec::new(),
            node_modules_path: None,
            force_nodejs_compat: false,
        }
    }
}

/// A module with resolved source code
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// Module name for imports
    pub name: String,
    /// Resolved JavaScript source code
    pub code: String,
    /// Whether to expose on globalThis
    pub expose_global: bool,
}

impl ResolvedModule {
    /// Resolve a module config to get the actual code
    pub async fn resolve(config: &ModuleConfig) -> Result<Self, std::io::Error> {
        let code = match &config.source {
            ModuleSource::Code(code) => code.clone(),
            ModuleSource::Path(path) => tokio::fs::read_to_string(path).await?,
        };
        Ok(Self {
            name: config.name.clone(),
            code,
            expose_global: config.expose_global,
        })
    }
}

/// Execute JavaScript code in an ephemeral workerd instance
///
/// This function:
/// 1. Generates a worker module with the user's code embedded
/// 2. Spawns a fresh workerd process
/// 3. Makes a single request to trigger execution
/// 4. Streams results back
/// 5. Tears down the workerd process
pub fn execute(
    code: String,
    context: ExecutionContext,
    _limits: ExecutionLimits,
    config: RunnerConfig,
) -> impl Stream<Item = JsExecEvent> + Send + 'static {
    stream! {
        // Transform static imports to dynamic imports
        let code = crate::import_transform::transform_imports(&code);

        let port = allocate_port();
        debug!(port = port, "Starting ephemeral worker");

        // Create temp directory for this execution
        let temp_dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(e) => {
                yield JsExecEvent::Error {
                    message: format!("Failed to create temp directory: {}", e),
                    name: "SystemError".to_string(),
                    location: None,
                    stack: vec![],
                };
                return;
            }
        };

        let module_path = temp_dir.path().join("worker.js");
        let config_path = temp_dir.path().join("config.capnp");

        // If node_modules_path is configured, symlink it into temp dir
        if let Some(ref nm_path) = config.node_modules_path {
            let target = temp_dir.path().join("node_modules");
            #[cfg(unix)]
            {
                if let Err(e) = std::os::unix::fs::symlink(nm_path, &target) {
                    yield JsExecEvent::Error {
                        message: format!("Failed to symlink node_modules: {}", e),
                        name: "SystemError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            }
            #[cfg(not(unix))]
            {
                // On Windows, try junction or copy as fallback
                if let Err(e) = std::fs::copy(nm_path, &target) {
                    yield JsExecEvent::Error {
                        message: format!("Failed to setup node_modules: {}", e),
                        name: "SystemError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            }
        }

        // Resolve all configured modules
        let mut resolved_modules = Vec::new();
        for module_config in &config.modules {
            match ResolvedModule::resolve(module_config).await {
                Ok(resolved) => resolved_modules.push(resolved),
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to resolve module '{}': {}", module_config.name, e),
                        name: "ModuleError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            }
        }

        // Write each module file to temp dir
        for module in &resolved_modules {
            let module_file = temp_dir.path().join(format!("{}.js", module.name));
            if let Err(e) = tokio::fs::write(&module_file, &module.code).await {
                yield JsExecEvent::Error {
                    message: format!("Failed to write module '{}': {}", module.name, e),
                    name: "SystemError".to_string(),
                    location: None,
                    stack: vec![],
                };
                return;
            }
        }

        // Generate the worker module with embedded code and module imports
        let module_content = generate_worker_module(&code, &context, &resolved_modules);
        if let Err(e) = tokio::fs::write(&module_path, &module_content).await {
            yield JsExecEvent::Error {
                message: format!("Failed to write worker module: {}", e),
                name: "SystemError".to_string(),
                location: None,
                stack: vec![],
            };
            return;
        }

        // Generate workerd config with module entries
        let has_node_modules = config.node_modules_path.is_some() || config.force_nodejs_compat;
        let workerd_config = generate_workerd_config(port, &resolved_modules, has_node_modules);
        if let Err(e) = tokio::fs::write(&config_path, &workerd_config).await {
            yield JsExecEvent::Error {
                message: format!("Failed to write config: {}", e),
                name: "SystemError".to_string(),
                location: None,
                stack: vec![],
            };
            return;
        }

        // Spawn workerd
        let mut process = match Command::new(&config.workerd_binary)
            .arg("serve")
            .arg(&config_path)
            .current_dir(temp_dir.path())
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(p) => p,
            Err(e) => {
                yield JsExecEvent::Error {
                    message: format!("Failed to spawn workerd: {}", e),
                    name: "SystemError".to_string(),
                    location: None,
                    stack: vec![],
                };
                return;
            }
        };

        // Wait for workerd to become healthy
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let health_url = format!("http://127.0.0.1:{}/health", port);
        let mut healthy = false;

        for attempt in 1..=config.health_check_attempts {
            // Check if process is still running
            match process.try_wait() {
                Ok(Some(status)) => {
                    // Process exited - capture stderr to get error details
                    let stderr = process.stderr.take();
                    let stderr_content = if let Some(mut stderr) = stderr {
                        use tokio::io::AsyncReadExt;
                        let mut buf = String::new();
                        let _ = stderr.read_to_string(&mut buf).await;
                        buf
                    } else {
                        String::new()
                    };

                    // Parse the error from stderr
                    let (name, message) = parse_workerd_error(&stderr_content, status);
                    yield JsExecEvent::Error {
                        message,
                        name,
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
                Ok(None) => {} // Still running
                Err(e) => {
                    yield JsExecEvent::Error {
                        message: format!("Failed to check process status: {}", e),
                        name: "SystemError".to_string(),
                        location: None,
                        stack: vec![],
                    };
                    return;
                }
            }

            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    trace!(attempt = attempt, "Health check passed");
                    healthy = true;
                    break;
                }
                _ => {
                    tokio::time::sleep(config.health_check_delay).await;
                }
            }
        }

        if !healthy {
            yield JsExecEvent::Error {
                message: "Workerd failed to start within timeout".to_string(),
                name: "SystemError".to_string(),
                location: None,
                stack: vec![],
            };
            return;
        }

        debug!(port = port, "Worker is healthy, executing code");

        // Make the execution request
        let execute_url = format!("http://127.0.0.1:{}/run", port);
        let response = match client.post(&execute_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                yield JsExecEvent::Error {
                    message: format!("Execution request failed: {}", e),
                    name: "NetworkError".to_string(),
                    location: None,
                    stack: vec![],
                };
                return;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            yield JsExecEvent::Error {
                message: format!("Execution failed with HTTP {}: {}", status, body),
                name: "ExecutionError".to_string(),
                location: None,
                stack: vec![],
            };
            return;
        }

        // Stream the NDJSON response
        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "Error reading response stream");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim();

                if !line.is_empty() {
                    if let Some(event) = parse_event(line) {
                        yield event;
                    }
                }

                buffer = buffer[newline_pos + 1..].to_string();
            }
        }

        // Process any remaining data
        let remaining = buffer.trim();
        if !remaining.is_empty() {
            if let Some(event) = parse_event(remaining) {
                yield event;
            }
        }

        debug!(port = port, "Execution complete, tearing down worker");
        // Process is killed on drop due to kill_on_drop(true)
    }
}

/// Parse a JSON line into a JsExecEvent
fn parse_event(line: &str) -> Option<JsExecEvent> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;

    let event_type = json.get("type")?.as_str()?;

    match event_type {
        "console" => {
            let level_str = json.get("level")?.as_str()?;
            let level = match level_str {
                "log" => LogLevel::Log,
                "info" => LogLevel::Info,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                "debug" => LogLevel::Debug,
                "trace" => LogLevel::Trace,
                _ => LogLevel::Log,
            };
            let args = json.get("args")?.as_array()?.clone();
            let timestamp_ms = json.get("timestamp_ms")?.as_u64()?;

            Some(JsExecEvent::Console {
                level,
                args,
                timestamp_ms,
            })
        }
        "returned" => {
            let value = json.get("value")?.clone();
            Some(JsExecEvent::Returned { value })
        }
        "error" => {
            let message = json.get("message")?.as_str()?.to_string();
            let name = json.get("name")?.as_str().unwrap_or("Error").to_string();

            let location = json.get("location").and_then(|loc| {
                if loc.is_null() { return None; }
                Some(SourceLocation {
                    line: loc.get("line")?.as_u64()? as u32,
                    column: loc.get("column")?.as_u64()? as u32,
                    filename: loc.get("filename").and_then(|f| f.as_str()).map(String::from),
                })
            });

            let stack = json.get("stack")
                .and_then(|s| s.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|frame| {
                        let function_name = frame.get("function_name")
                            .and_then(|f| f.as_str())
                            .map(String::from);
                        let loc = frame.get("location").and_then(|loc| {
                            if loc.is_null() { return None; }
                            Some(SourceLocation {
                                line: loc.get("line")?.as_u64()? as u32,
                                column: loc.get("column")?.as_u64()? as u32,
                                filename: loc.get("filename").and_then(|f| f.as_str()).map(String::from),
                            })
                        })?;
                        Some(StackFrame { function_name, location: loc })
                    }).collect()
                })
                .unwrap_or_default();

            Some(JsExecEvent::Error {
                message,
                name,
                location,
                stack,
            })
        }
        _ => None,
    }
}

/// Parse workerd stderr to determine error type and message
fn parse_workerd_error(stderr: &str, status: std::process::ExitStatus) -> (String, String) {
    // Look for common error patterns in workerd output
    let stderr_lower = stderr.to_lowercase();

    // Check for syntax errors
    if stderr_lower.contains("syntaxerror") || stderr_lower.contains("syntax error") {
        // Try to extract the actual error message
        let message = extract_js_error_message(stderr)
            .unwrap_or_else(|| format!("Syntax error in code: {}", truncate_stderr(stderr)));
        return ("SyntaxError".to_string(), message);
    }

    // Check for reference errors
    if stderr_lower.contains("referenceerror") {
        let message = extract_js_error_message(stderr)
            .unwrap_or_else(|| format!("Reference error: {}", truncate_stderr(stderr)));
        return ("ReferenceError".to_string(), message);
    }

    // Check for type errors
    if stderr_lower.contains("typeerror") {
        let message = extract_js_error_message(stderr)
            .unwrap_or_else(|| format!("Type error: {}", truncate_stderr(stderr)));
        return ("TypeError".to_string(), message);
    }

    // Default to system error with stderr content
    let message = if stderr.is_empty() {
        format!("Workerd exited with status: {}", status)
    } else {
        format!("Workerd error: {}", truncate_stderr(stderr))
    };

    ("SystemError".to_string(), message)
}

/// Try to extract a JS error message from workerd stderr
fn extract_js_error_message(stderr: &str) -> Option<String> {
    // workerd outputs errors like: "worker.js:5:12: SyntaxError: Unexpected token ')'"
    // or multi-line with the error on its own line
    for line in stderr.lines() {
        let line = line.trim();
        // Look for lines containing error messages
        if line.contains("SyntaxError:") {
            if let Some(pos) = line.find("SyntaxError:") {
                return Some(line[pos..].to_string());
            }
        }
        if line.contains("ReferenceError:") {
            if let Some(pos) = line.find("ReferenceError:") {
                return Some(line[pos..].to_string());
            }
        }
        if line.contains("TypeError:") {
            if let Some(pos) = line.find("TypeError:") {
                return Some(line[pos..].to_string());
            }
        }
    }
    None
}

/// Truncate stderr for error messages
fn truncate_stderr(stderr: &str) -> String {
    let s = stderr.trim();
    if s.len() > 200 {
        format!("{}...", &s[..200])
    } else {
        s.to_string()
    }
}

/// Generate the worker module with user code embedded
fn generate_worker_module(code: &str, context: &ExecutionContext, modules: &[ResolvedModule]) -> String {
    let context_json = serde_json::to_string(context).unwrap_or_else(|_| "{}".to_string());

    // Generate import statements for preloaded modules
    let imports: String = modules
        .iter()
        .map(|m| format!("import * as {} from '{}';", m.name, m.name))
        .collect::<Vec<_>>()
        .join("\n");

    // Generate globalThis assignments for modules that want to be exposed
    let global_exports: String = modules
        .iter()
        .filter(|m| m.expose_global)
        .map(|m| format!("globalThis.{name} = {name}.default || {name};", name = m.name))
        .collect::<Vec<_>>()
        .join("\n");

    // Create a modules object that user code can access
    let modules_object: String = if modules.is_empty() {
        "{}".to_string()
    } else {
        let entries: Vec<String> = modules
            .iter()
            .map(|m| format!("  {name}: {name}.default || {name}", name = m.name))
            .collect();
        format!("{{\n{}\n}}", entries.join(",\n"))
    };

    format!(r#"
// Auto-generated worker module for jsexec
// User code is embedded directly - no eval required

{imports}

// Expose modules on globalThis if configured
{global_exports}

const CTX = {context_json};

// All loaded modules available as ctx.modules
const MODULES = {modules_object};

// Console wrapper that collects output
function createConsole(writer, encoder) {{
    const emit = async (level, args) => {{
        const event = {{
            type: 'console',
            level,
            args: args.map(arg => {{
                try {{
                    if (arg === undefined) return null;
                    if (arg instanceof Error) {{
                        return {{ message: arg.message, name: arg.name }};
                    }}
                    return JSON.parse(JSON.stringify(arg));
                }} catch {{
                    return String(arg);
                }}
            }}),
            timestamp_ms: Date.now()
        }};
        await writer.write(encoder.encode(JSON.stringify(event) + '\n'));
    }};

    return {{
        log: (...args) => emit('log', args),
        info: (...args) => emit('info', args),
        warn: (...args) => emit('warn', args),
        error: (...args) => emit('error', args),
        debug: (...args) => emit('debug', args),
        trace: (...args) => emit('trace', args),
    }};
}}

// Main execution function
async function executeUserCode(writer, encoder, console) {{
    const ctx = {{ ...CTX.data, modules: MODULES }};

    try {{
        // User code runs here directly
        const __result = await (async () => {{
            {code}
        }})();

        // Emit the result
        const event = {{
            type: 'returned',
            value: __result === undefined ? null : __result
        }};
        await writer.write(encoder.encode(JSON.stringify(event) + '\n'));
    }} catch (error) {{
        // Emit the error
        const event = {{
            type: 'error',
            message: error.message || String(error),
            name: error.name || 'Error',
            location: null,
            stack: []
        }};
        await writer.write(encoder.encode(JSON.stringify(event) + '\n'));
    }}
}}

export default {{
    async fetch(request) {{
        const url = new URL(request.url);

        // Health check
        if (url.pathname === '/health') {{
            return new Response(JSON.stringify({{ status: 'ok' }}), {{
                headers: {{ 'Content-Type': 'application/json' }}
            }});
        }}

        // Run the code
        if (url.pathname === '/run') {{
            const {{ readable, writable }} = new TransformStream();
            const writer = writable.getWriter();
            const encoder = new TextEncoder();
            const console = createConsole(writer, encoder);

            // Execute in background, close writer when done
            (async () => {{
                try {{
                    await executeUserCode(writer, encoder, console);
                }} finally {{
                    await writer.close();
                }}
            }})();

            return new Response(readable, {{
                headers: {{ 'Content-Type': 'application/x-ndjson' }}
            }});
        }}

        return new Response('Not found', {{ status: 404 }});
    }}
}};
"#)
}

/// Generate workerd config with module entries
fn generate_workerd_config(port: u16, modules: &[ResolvedModule], has_node_modules: bool) -> String {
    // Generate module entries for the Cap'n Proto config
    // The main worker module is always first, then preloaded modules
    let mut module_entries = vec![
        r#"    (name = "worker", esModule = embed "worker.js")"#.to_string()
    ];

    // Add each preloaded module as an ES module
    for module in modules {
        module_entries.push(format!(
            r#"    (name = "{name}", esModule = embed "{name}.js")"#,
            name = module.name
        ));
    }

    let modules_list = module_entries.join(",\n");

    // Add nodejs_compat if node_modules is available
    let compat_flags = if has_node_modules {
        r#"
  compatibilityFlags = ["nodejs_compat"],"#
    } else {
        ""
    };

    format!(r#"using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [
    (name = "main", worker = .mainWorker),
  ],
  sockets = [
    (
      name = "http",
      address = "127.0.0.1:{port}",
      http = (),
      service = "main"
    ),
  ]
);

const mainWorker :Workerd.Worker = (
  modules = [
{modules_list}
  ],
  compatibilityDate = "2024-01-01",{compat_flags}
);
"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_worker_module() {
        let code = "return 1 + 1";
        let context = ExecutionContext::default();
        let modules = vec![];
        let module = generate_worker_module(code, &context, &modules);

        assert!(module.contains("return 1 + 1"));
        assert!(module.contains("/health"));
        assert!(module.contains("/run"));
    }

    #[test]
    fn test_generate_worker_module_with_modules() {
        let code = "return ctx.modules.utils.add(1, 2)";
        let context = ExecutionContext::default();
        let modules = vec![
            ResolvedModule {
                name: "utils".to_string(),
                code: "export function add(a, b) { return a + b; }".to_string(),
                expose_global: true,
            },
        ];
        let module = generate_worker_module(code, &context, &modules);

        // Check imports are generated
        assert!(module.contains("import * as utils from 'utils';"));
        // Check globalThis exposure
        assert!(module.contains("globalThis.utils = utils.default || utils;"));
        // Check modules object
        assert!(module.contains("utils: utils.default || utils"));
    }

    #[test]
    fn test_generate_workerd_config() {
        let modules = vec![];
        let config = generate_workerd_config(8787, &modules, false);
        assert!(config.contains("127.0.0.1:8787"));
        assert!(config.contains("worker.js"));
        assert!(!config.contains("nodejs_compat"));
    }

    #[test]
    fn test_generate_workerd_config_with_modules() {
        let modules = vec![
            ResolvedModule {
                name: "lodash".to_string(),
                code: "export default {}".to_string(),
                expose_global: false,
            },
        ];
        let config = generate_workerd_config(8787, &modules, false);
        assert!(config.contains("worker.js"));
        assert!(config.contains(r#"(name = "lodash", esModule = embed "lodash.js")"#));
    }

    #[test]
    fn test_generate_workerd_config_with_node_modules() {
        let modules = vec![];
        let config = generate_workerd_config(8787, &modules, true);
        assert!(config.contains("worker.js"));
        assert!(config.contains(r#"compatibilityFlags = ["nodejs_compat"]"#));
    }

    #[test]
    fn test_port_allocation() {
        let p1 = allocate_port();
        let p2 = allocate_port();
        assert_ne!(p1, p2);
        assert_eq!(p2, p1 + 1);
    }
}
