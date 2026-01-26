# JsExec Handoff Document

## What This Is

JsExec is a Plexus plugin that executes JavaScript code in sandboxed V8 isolates using Cloudflare's `workerd` runtime.

## Architecture (Ephemeral Model)

Each execution spawns a **fresh workerd process** with the user's code embedded directly as the worker module. No `eval()` needed.

```
User calls jsexec.evaluate("1 + 1")
    ↓
Generate worker.js with code embedded:
    const __result = await (async () => {
        return (1 + 1)
    })();
    ↓
Write worker.js + config.capnp to temp dir
    ↓
Spawn: workerd serve config.capnp
    ↓
Wait for /health to respond
    ↓
GET /run → streams NDJSON events
    ↓
Tear down workerd (kill_on_drop)
```

## Current File Structure

```
jsexec/
├── src/
│   ├── lib.rs           # Crate entry, re-exports
│   ├── types.rs         # JsExecEvent, JsExecConfig, etc.
│   ├── activation.rs    # JsExec with #[hub_methods] macro
│   ├── runner.rs        # Ephemeral execution (spawn per request)
│   └── serde_helpers.rs # Re-export for hub_macro
├── tests/
│   └── integration.rs   # 10 integration tests
└── docs/
    └── handoff.md       # This file
```

## Current Status: ALL TESTS PASSING

All 23 tests pass:
- 12 unit tests
- 10 integration tests
- 1 doc test

```
running 10 tests
test test_syntax_error ... ok
test test_array_return ... ok
test test_math_operations ... ok
test test_runtime_error ... ok
test test_null_return ... ok
test test_boolean_logic ... ok
test test_object_return ... ok
test test_run_code_directly ... ok
test test_simple_eval ... ok
test test_console_log ... ok

test result: ok. 10 passed; 0 failed
```

## Key Implementation Details

### `src/runner.rs` - Ephemeral Execution

Core function `execute()` that:
1. Allocates a unique port
2. Creates temp directory
3. Generates worker module with embedded code
4. Spawns workerd process
5. Waits for health check
6. Makes HTTP request to `/run`
7. Streams NDJSON events back

Key features:
- **Error parsing**: `parse_workerd_error()` captures stderr and detects SyntaxError, ReferenceError, TypeError
- **Console capture**: Custom console wrapper emits NDJSON events
- **Port allocation**: Atomic counter prevents port conflicts

### `src/activation.rs` - Public API

Two main methods:
- `run(code)` - Execute arbitrary JS code (can use `return`)
- `evaluate(expr)` - Evaluate an expression, wraps with `return (expr)`

### API Usage Notes

- Use `run()` for multi-statement code with explicit `return`
- Use `evaluate()` for single expressions (objects, calculations)
- For multi-expression with result, use comma operator: `evaluate("(console.log('hi'), 42)")`

## What Was Fixed (Latest Session)

1. **test_math_operations**: Changed assertion to compare as f64 (12 vs 12.0)
2. **test_console_log**: Changed from `evaluate()` to `run()` since multi-statement code can't be wrapped as expression
3. **test_runtime_error**: Changed to `run()` since `throw` is a statement, not expression
4. **test_syntax_error**: Added stderr capture to parse actual error type from workerd output
5. **Warnings**: Cleaned up all unused imports

## How to Test

```bash
cd /Users/user/dev/controlflow/hypermemetic/jsexec

# Run all tests
cargo test

# Run integration tests only
cargo test --test integration

# Run a single test
cargo test --test integration test_simple_eval
```

## Dependencies

- `workerd` must be in PATH (installed via `npm install -g workerd`)
- Uses hub-core and hub-macro from the parent substrate project

## Potential Future Work

1. **Resource limits**: The `_limits` parameter is currently unused. Could implement CPU/memory limits via workerd config.
2. **Context data**: `ExecutionContext.data` is passed but not heavily tested.
3. **Bindings**: `BindingConfig` types are defined but not yet implemented.
4. **Pool mode**: For high-throughput scenarios, could add optional worker pooling.
