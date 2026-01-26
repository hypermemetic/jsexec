# JsExec

A Plexus plugin for executing JavaScript code in sandboxed V8 isolates using Cloudflare's `workerd` runtime.

## Overview

JsExec provides secure JavaScript execution with:
- **Sandboxed V8 isolates** via Cloudflare workerd
- **Ephemeral execution model** - fresh process per execution, no state leakage
- **Streaming output** - console logs and results stream back in real-time
- **Script storage** - save and rerun scripts by ID
- **No eval required** - user code is embedded directly in the worker module

## Installation

### Prerequisites

```bash
# Install workerd (Cloudflare's runtime)
npm install -g workerd

# Verify installation
workerd --version
```

### As a Plexus Plugin

JsExec is registered in `substrate/src/builder.rs`:

```rust
use jsexec::{JsExec, JsExecConfig};

Plexus::new()
    .register(JsExec::new(JsExecConfig::default()))
    // ...
```

## API Reference

### `jsexec.execute`

Execute JavaScript code with full statement support.

**Parameters:**
- `code` (string): JavaScript source code to execute

**Returns:** Stream of `JsExecEvent`

**Example:**
```javascript
// Via MCP
jsexec.execute({
  code: `
    const x = 10;
    const y = 20;
    console.log("Sum:", x + y);
    return { sum: x + y, product: x * y };
  `
})

// Output:
// { type: "console", level: "log", args: ["Sum:", 30], timestamp_ms: ... }
// { type: "returned", value: { sum: 30, product: 200 } }
```

### `jsexec.eval`

Evaluate a JavaScript expression and return the result. The expression is wrapped with `return (expr)`.

**Parameters:**
- `expr` (string): JavaScript expression to evaluate

**Returns:** Stream of `JsExecEvent`

**Example:**
```javascript
jsexec.eval({ expr: "Math.sqrt(16) + Math.pow(2, 3)" })
// { type: "returned", value: 12 }

jsexec.eval({ expr: '({ name: "test", values: [1, 2, 3] })' })
// { type: "returned", value: { name: "test", values: [1, 2, 3] } }
```

**Note:** Use `execute()` for multi-statement code. `eval()` is for single expressions only.

### `jsexec.store`

Store a script for later execution.

**Parameters:**
- `name` (string): Human-readable name for the script
- `code` (string): JavaScript source code
- `description` (string, optional): Description of what the script does

**Returns:** `ScriptStored` event with script ID

**Example:**
```javascript
jsexec.store({
  name: "fibonacci",
  code: `
    function fib(n) {
      if (n <= 1) return n;
      return fib(n - 1) + fib(n - 2);
    }
    return fib(10);
  `,
  description: "Calculate 10th Fibonacci number"
})
// { type: "script_stored", script_id: "...", name: "fibonacci", ... }
```

### `jsexec.execute_script`

Run a previously stored script by ID.

**Parameters:**
- `script_id` (UUID): ID of the stored script

**Returns:** Stream of `JsExecEvent`

### `jsexec.list_scripts`

List all stored scripts.

**Returns:** Stream of `ScriptMetadata`

### `jsexec.delete_script`

Delete a stored script.

**Parameters:**
- `script_id` (UUID): ID of the script to delete

**Returns:** `ScriptDeleted` event

## Event Types

### `JsExecEvent`

All execution methods return a stream of these events:

```typescript
type JsExecEvent =
  | { type: "console", level: LogLevel, args: any[], timestamp_ms: number }
  | { type: "returned", value: any }
  | { type: "error", message: string, name: string, location?: SourceLocation, stack: StackFrame[] }
  | { type: "script_stored", script_id: string, name: string, size_bytes: number, hash: string }
  | { type: "script_deleted", script_id: string }

type LogLevel = "log" | "info" | "warn" | "error" | "debug" | "trace"
```

## Architecture

```
jsexec.execute("1 + 1")
    |
    v
Generate worker.js with embedded code:
    const __result = await (async () => {
        return (1 + 1)
    })();
    |
    v
Write worker.js + config.capnp to temp dir
    |
    v
Spawn: workerd serve config.capnp
    |
    v
Wait for /health endpoint
    |
    v
POST /run -> streams NDJSON events
    |
    v
Tear down workerd (kill_on_drop)
```

### Why Ephemeral?

Each execution spawns a fresh workerd process because:

1. **Security** - No state leakage between executions
2. **Simplicity** - No eval/dynamic code needed (code IS the module)
3. **Isolation** - Each execution is completely independent
4. **Fast startup** - workerd starts in ~50-100ms

## Configuration

```rust
pub struct JsExecConfig {
    /// Default execution limits (currently unused, reserved for future)
    pub default_limits: ExecutionLimits,
    /// Enable console output capture (default: true)
    pub capture_console: bool,
}

pub struct ExecutionLimits {
    pub cpu_time_ms: Option<u64>,
    pub wall_time_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub max_fetch_requests: Option<u32>,
    pub max_response_bytes: Option<u64>,
}
```

## Error Handling

JsExec detects and reports different error types:

### Syntax Errors
```javascript
jsexec.eval({ expr: "function(" })
// { type: "error", name: "SyntaxError", message: "Unexpected token..." }
```

### Runtime Errors
```javascript
jsexec.execute({ code: "throw new Error('oops')" })
// { type: "error", name: "Error", message: "oops" }
```

### Reference Errors
```javascript
jsexec.eval({ expr: "undefinedVariable" })
// { type: "error", name: "ReferenceError", message: "undefinedVariable is not defined" }
```

## Console API

The full console API is available:

```javascript
jsexec.execute({
  code: `
    console.log("Log message");
    console.info("Info message");
    console.warn("Warning message");
    console.error("Error message");
    console.debug("Debug message");
    console.trace("Trace message");
    return "done";
  `
})
```

Each console call emits a separate event before the final result.

## Limitations

- **No network access** - fetch() is not available (can be added via bindings)
- **No filesystem access** - scripts run in isolated sandbox
- **No persistent state** - each execution starts fresh
- **Expression vs Statement** - use `eval()` for expressions, `execute()` for statements

## Development

```bash
cd /path/to/jsexec

# Run tests
cargo test

# Run integration tests (requires workerd)
cargo test --test integration

# Build
cargo build
```

## License

AGPL-3.0-only
