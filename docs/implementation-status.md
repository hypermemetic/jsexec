# JsExec Implementation Status

## Overview

JsExec is a Plexus plugin that executes JavaScript code in sandboxed V8 isolates using Cloudflare's `workerd` runtime. The goal is to provide secure, isolated JavaScript execution with streaming output.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      JsExec Activation                       │
│  (Plexus plugin with hub_methods: execute, eval, store...)  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                       PoolManager                            │
│  - Manages pool of workerd processes                         │
│  - Semaphore-based concurrency control                       │
│  - WorkerGuard for RAII acquisition/release                  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                         Worker                               │
│  - Wraps a single workerd process                            │
│  - HTTP client for /health and /execute endpoints            │
│  - Streams NDJSON events back to caller                      │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    workerd process                           │
│  Running executor.js which:                                  │
│  - Exposes /health endpoint                                  │
│  - Exposes /execute endpoint that runs JS code               │
│  - Streams console output and return values as NDJSON        │
└─────────────────────────────────────────────────────────────┘
```

## Current File Structure

```
jsexec/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Crate entry point, re-exports
│   ├── types.rs            # JsExecEvent, JsExecConfig, limits, etc.
│   ├── activation.rs       # JsExec with #[hub_methods] macro
│   ├── serde_helpers.rs    # Re-export of plexus_core serde helpers
│   ├── pool/
│   │   ├── mod.rs
│   │   ├── worker.rs       # Worker struct wrapping workerd process
│   │   └── manager.rs      # PoolManager for worker pool
│   └── runtime/
│       ├── mod.rs          # Embeds executor.js at compile time
│       └── executor.js     # JavaScript worker that runs inside workerd
└── tests/
    └── integration.rs      # Integration tests (18 tests)
```

## How It Works

1. **Startup**: `JsExec::new()` creates a `PoolManager` which spawns N workerd processes
2. **Each workerd process**:
   - Gets a temporary directory with `config.capnp` and `executor.js`
   - Listens on a unique port (base_port + worker_index)
   - Runs the executor.js worker code
3. **Code Execution**:
   - Client calls `jsexec.evaluate("1 + 1")`
   - Code is wrapped as `return (1 + 1)`
   - POST to workerd's `/execute` endpoint with `{"code": "return (1 + 1)"}`
   - executor.js creates an AsyncFunction from the code and runs it
   - Results stream back as NDJSON events

## Event Types

```rust
enum JsExecEvent {
    ExecutionStarted { execution_id, script_id, worker_id },
    ExecutionCompleted { execution_id, metrics },
    Console { level, args, timestamp_ms },
    Returned { value },
    Error { message, name, location, stack },
    // ... more
}
```

## Current Issue: Dynamic Code Execution

The executor.js uses `new AsyncFunction()` to dynamically execute user code:

```javascript
const AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;
const fn = new AsyncFunction('console', 'ctx', code);
const result = await fn(wrappedConsole, context);
```

**Problem**: Cloudflare Workers (and workerd) disable dynamic code generation by default for security. This includes:
- `eval()`
- `new Function()`
- `new AsyncFunction()`

**Error**: `"Code generation from strings disallowed for this context"`

### Solutions

1. **Enable unsafe_eval binding** (requires `--experimental` flag):
   ```capnp
   bindings = [(name = "unsafeEval", unsafeEval = void)]
   ```
   Then run: `workerd serve config.capnp --experimental`

2. **Use a different execution model**:
   - Pre-compile code to a module format
   - Use a JavaScript interpreter written in JS (e.g., JS-Interpreter)
   - Use QuickJS/Duktape via WASM

## What Works

- ✅ Crate structure and compilation
- ✅ Type definitions (events, configs, limits)
- ✅ Worker spawning and health checks
- ✅ Pool manager with concurrency control
- ✅ Activation with hub_methods macro
- ✅ Integration tests compile and run
- ✅ Workerd starts and responds to /health

## What Doesn't Work Yet

- ❌ Actual code execution (blocked by unsafe_eval restriction)
- ❌ Integration tests pass (no results returned)

## Next Steps

### Option A: Use --experimental flag
Modify worker spawning to include `--experimental`:
```rust
Command::new(&config.workerd_binary)
    .arg("serve")
    .arg(&config_path)
    .arg("--experimental")  // Add this
    .spawn()?;
```

And update the workerd config generation:
```rust
fn generate_workerd_config(port: u16) -> String {
    format!(r#"
const jsexecWorker :Workerd.Worker = (
  modules = [...],
  compatibilityDate = "2024-01-01",
  bindings = [(name = "unsafeEval", unsafeEval = void)]
);
"#)
}
```

### Option B: Alternative execution model
Use a JavaScript-based interpreter that doesn't require eval. This would be safer but slower.

## Configuration

```rust
pub struct JsExecConfig {
    pub min_workers: usize,      // Default: 1
    pub max_workers: usize,      // Default: 4
    pub base_port: u16,          // Default: 8787
    pub default_limits: ExecutionLimits,
    pub idle_timeout_ms: u64,    // Default: 30000
    pub max_executions_per_worker: u64,  // Default: 1000
    pub enable_metrics: bool,    // Default: true
    pub capture_console: bool,   // Default: true
}
```

## Dependencies

- `hub-core` / `hub-macro`: Plexus plugin infrastructure
- `workerd`: Cloudflare Workers runtime (must be in PATH)
- `reqwest`: HTTP client for worker communication
- `tokio`: Async runtime
- `async-stream`: For streaming event generation
