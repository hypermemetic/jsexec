# plexus_env — Progress Report

## What It Is

A new `plexus_env` RPC method for jsexec that generates typed TypeScript clients for Plexus backends at runtime. Pipeline: `synapse -i → IR JSON → hub-codegen → TypeScript → esbuild → single .js → workerd module`.

User code gets a fully typed Plexus client injected as `ctx.modules.plexus` with the connection URL at `ctx.data.plexus_url`.

## What's Done

### Code written and compiling

All source changes are in place and `cargo build` succeeds cleanly.

**New file — `jsexec/src/plexus_env.rs`:**
- `ToolPaths` struct, `BundleSource` enum
- `discover_tools()` — searches explicit config → `~/.cabal/bin`, `~/.cargo/bin`, `~/.plexus/bin` → PATH via `which`
- `generate_ir()` — runs `synapse -H -P -i`, extracts `irHash` from JSON
- `generate_typescript()` — runs `hub-codegen --target typescript --bundle-transport=true`
- `bundle_to_esm()` — runs `esbuild --bundle --format=esm --external:ws --minify`
- `PlexusClientCache` — file-based cache at `~/.cache/jsexec/plexus/`, keyed by `{backend}_{irHash}.js`
- `get_plexus_client_bundle()` — full orchestrator with cache
- `WS_SHIM` constant — re-exports `globalThis.WebSocket` as the `ws` npm package

**Modified files:**
- `Cargo.toml` — added `which = "7"`, `dirs = "6"`
- `types.rs` — added `PlexusEnvProgress { stage, message }` variant to `JsExecEvent`; added `synapse_path`, `hub_codegen_path`, `plexus_cache_dir` to `JsExecConfig`
- `runner.rs` — added `force_nodejs_compat: bool` to `RunnerConfig`, `has_node_modules` check now respects it
- `activation.rs` — added `plexus_env` hub method (streams progress events, builds ws shim + plexus modules, injects `plexus_url`, runs with `force_nodejs_compat: true`)
- `lib.rs` — registered `pub mod plexus_env`

### Tests passing

6 new unit tests for cache and ws shim all pass. All pre-existing tests unaffected (6 `lambda::metrics` failures are pre-existing SQLite schema issues, not related).

## What Remains

### 1. Build the tool binaries

The pipeline depends on three external binaries that aren't installed yet:

| Binary | Source | How to build |
|---|---|---|
| `hub-codegen` | `hub-codegen/` (Rust, has `Cargo.toml`) | `cd hub-codegen && cargo build --release`, then put on PATH or set `hub_codegen_path` in config |
| `esbuild` | npm | `npm install -g esbuild` or use one from a `node_modules/.bin/` |
| `workerd` | npm / brew | `npm install -g workerd` or build from source |

`synapse` is already installed at `~/.local/bin/synapse`. `synapse-cc` is built at `synapse-cc/dist-newstyle/...` but `hub-codegen` is the Rust codegen tool, not the Haskell one.

### 2. Verify hub-codegen CLI interface

The `generate_typescript()` function assumes this CLI interface:
```
hub-codegen --target typescript --output <dir> --bundle-transport=true <ir-file>
```
Need to confirm the actual flags by reading `hub-codegen/src/main.rs` or `--help`. If the flags differ, update `plexus_env.rs`.

### 3. Verify synapse IR JSON shape

The code expects `synapse -i <backend>` to produce JSON with an `irHash` field at the top level. Need to confirm by running:
```bash
synapse -H 127.0.0.1 -P 4444 -i substrate | head -20
```
If the hash field has a different name or location, update `generate_ir()`.

### 4. Verify generated TypeScript entry point

`bundle_to_esm()` expects `hub-codegen` to produce an `index.ts` at the root of the output directory. If the entry point has a different name or is nested, update the path.

### 5. Verify the ws shim works in workerd

The generated transport code does `import WebSocket from 'ws'`. The ws shim re-exports `globalThis.WebSocket`. This needs to be tested in an actual workerd instance with `nodejs_compat` enabled to confirm:
- workerd exposes `globalThis.WebSocket` with nodejs_compat
- The shim correctly satisfies the import

### 6. End-to-end integration test

Once all binaries are available:
```bash
synapse substrate jsexec plexus_env \
  --host 127.0.0.1 --port 4444 --backend substrate \
  --code "const { createClient, createEchoClient } = ctx.modules.plexus; \
          const c = createClient({url: ctx.data.plexus_url}); \
          await c.connect(); \
          const echo = createEchoClient(c); \
          const r = await echo.once('test'); \
          c.disconnect(); \
          return r;"
```

Verify:
- Progress events stream (`discovering_tools`, `generating_ir`, `generating_code`, `bundling`)
- Second run shows `cache_hit` event
- Return value matches echo input

### 7. Error path testing

- Missing binary → clear error message with search paths listed
- Wrong host/port → synapse failure propagated
- Backend with no methods → codegen produces valid empty client
- Network timeout during IR fetch

## Architecture Decisions

- **File cache, not memory cache** — bundles survive process restarts, keyed by IR content hash so they auto-invalidate when the backend schema changes
- **ws shim over bundling ws** — workerd has native WebSocket via nodejs_compat, so we inject a shim module rather than bundling the entire `ws` npm package
- **force_nodejs_compat** — new RunnerConfig flag avoids requiring a dummy node_modules directory just to get WebSocket support
- **Progress events** — `PlexusEnvProgress` variant lets callers show pipeline stages without parsing log output
