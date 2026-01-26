# JsExec Module Loading and NPM Bundling Architecture

## Overview

JsExec provides JavaScript execution in sandboxed V8 isolates using Cloudflare's workerd runtime. This document covers the module loading system and NPM package bundling capabilities added to support flexible code composition and dependency management.

## Problem Space

### Workerd Module Loading Limitations

Workerd is designed for edge computing with pre-bundled code. Unlike Node.js, it has no automatic filesystem-based module resolution:

- **No automatic node_modules scanning**: Cannot use `import 'typescript'` directly
- **Explicit module declaration**: All modules must be declared in Cap'n Proto config
- **Static configuration**: Modules defined at process startup, not dynamically

This creates challenges when users want to:
1. Load custom utility modules alongside their execution code
2. Use npm packages from node_modules
3. Share code between multiple executions

## Solution Architecture

### 1. Path-Based Module Loading

#### Design

Allow users to load JavaScript modules by providing file paths. The system automatically:
- Reads the file content
- Derives a valid JavaScript identifier from the filename
- Injects the module into workerd's configuration

#### Implementation

**API Surface:**

```rust
// Public API
jsexec.run_with_modules(code, vec![
    "/path/to/helpers.js".to_string(),
    "/path/to/utils.js".to_string(),
])

// RPC API
jsexec.execute_with_modules(code, module_paths)
```

**Module Name Derivation** (`types.rs:ModuleConfig::from_path()`):

```rust
pub fn from_path(path: impl Into<PathBuf>) -> Self {
    let path_buf = path.into();
    let name = path_buf
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .replace('-', "_");  // Critical: dashes aren't valid in JS identifiers
    Self {
        name,
        source: ModuleSource::Path(path_buf),
        expose_global: false,
    }
}
```

**Why dash → underscore conversion?**

The generated workerd config includes:
```javascript
import * as <name> from '<name>';
```

Since `<name>` appears as a JavaScript identifier, it must follow identifier rules (no dashes).

**Example:**
- File: `tools-simple.js`
- Derived name: `tools_simple`
- Generated: `import * as tools_simple from 'tools_simple';`
- Usage in code: `tools_simple.someFunction()`

#### Usage Pattern

```rust
// Create JsExec instance
let jsexec = JsExec::new(JsExecConfig::default());

// Load helper modules by path
let helpers = vec![
    "./scripts/math-helpers.js".to_string(),
    "./scripts/formatters.js".to_string(),
];

// Execute with modules available
let code = r#"
    import { add } from 'math_helpers';  // Note underscore
    import { format } from 'formatters';

    const result = add(1, 2);
    return format(result);
"#;

let stream = jsexec.run_with_modules(code.to_string(), helpers);
```

### 2. NPM Package Auto-Bundling

#### The Problem

Users want to use npm packages like TypeScript, lodash, etc. Workerd cannot auto-resolve these from node_modules, so we need to bundle them into single-file ES modules.

#### Design Philosophy

**On-Demand Bundling:**
- Bundle packages only when needed for execution
- Use esbuild for fast, reliable bundling
- Support caching to avoid redundant bundling

**Cache Abstraction:**
- Define a trait for flexible caching strategies
- Don't prescribe implementation (in-memory, disk, Redis, S3)
- Allow users to bring their own cache backend

#### Implementation

**Bundle Cache Trait** (`bundler.rs`):

```rust
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
```

**Why async?**
- Supports network-based caches (Redis, S3)
- Allows I/O without blocking
- Composable with tokio runtime

**No-Op Cache** (for when caching is disabled):

```rust
pub struct NoCache;

#[async_trait]
impl BundleCache for NoCache {
    async fn get(&self, _key: &str) -> Option<String> {
        None  // Never cached
    }

    async fn put(&self, _key: &str, _content: String) {
        // No-op
    }
}
```

**Bundling Process** (`bundler.rs:bundle_package()`):

1. **Create temporary entry point**:
   ```javascript
   export * from 'typescript';
   import pkg from 'typescript';
   export default pkg;
   ```

2. **Run esbuild**:
   ```bash
   esbuild entry.js --bundle --format=esm --platform=browser
   ```

3. **Capture stdout** as bundled module code

4. **Return bundled string** ready for workerd

**Caching Wrapper** (`bundler.rs:bundle_package_cached()`):

```rust
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
```

#### Cache Implementation Examples

**In-Memory Cache:**

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub struct MemoryCache {
    data: Arc<RwLock<HashMap<String, String>>>,
}

#[async_trait]
impl BundleCache for MemoryCache {
    async fn get(&self, key: &str) -> Option<String> {
        self.data.read().await.get(key).cloned()
    }

    async fn put(&self, key: &str, content: String) {
        self.data.write().await.insert(key.to_string(), content);
    }
}
```

**Filesystem Cache:**

```rust
use std::path::PathBuf;
use tokio::fs;

pub struct FileCache {
    cache_dir: PathBuf,
}

#[async_trait]
impl BundleCache for FileCache {
    async fn get(&self, key: &str) -> Option<String> {
        let path = self.cache_dir.join(format!("{}.bundle.js", key));
        fs::read_to_string(path).await.ok()
    }

    async fn put(&self, key: &str, content: String) {
        let path = self.cache_dir.join(format!("{}.bundle.js", key));
        let _ = fs::write(path, content).await;
    }

    fn cache_key(&self, package_name: &str, node_modules: &Path) -> String {
        // Include version hash in key
        let package_json = node_modules.join(package_name).join("package.json");
        // ... read version, compute hash
        format!("{}@{}", package_name, version_hash)
    }
}
```

**Redis Cache:**

```rust
use redis::AsyncCommands;

pub struct RedisCache {
    client: redis::Client,
}

#[async_trait]
impl BundleCache for RedisCache {
    async fn get(&self, key: &str) -> Option<String> {
        let mut conn = self.client.get_async_connection().await.ok()?;
        conn.get(format!("bundle:{}", key)).await.ok()
    }

    async fn put(&self, key: &str, content: String) {
        if let Ok(mut conn) = self.client.get_async_connection().await {
            let _: Result<(), _> = conn.set_ex(
                format!("bundle:{}", key),
                content,
                86400  // 24 hour TTL
            ).await;
        }
    }
}
```

### 3. Configuration

**JsExecConfig** (`types.rs`):

```rust
pub struct JsExecConfig {
    /// Default execution limits
    pub default_limits: ExecutionLimits,

    /// Whether to capture console output
    pub capture_console: bool,

    /// Preloaded modules available to all executions
    pub modules: Vec<ModuleConfig>,

    /// Path to node_modules directory for npm packages
    pub node_modules_path: Option<PathBuf>,

    /// NPM packages to auto-bundle and load
    pub npm_packages: Vec<String>,

    /// Path to esbuild binary (defaults to "esbuild" in PATH)
    pub esbuild_path: Option<PathBuf>,
}
```

**BundleConfig** (`bundler.rs`):

```rust
pub struct BundleConfig {
    /// Path to esbuild binary
    pub esbuild_path: PathBuf,

    /// Path to node_modules directory
    pub node_modules_path: PathBuf,

    /// Output format (ESM or CJS)
    pub format: BundleFormat,
}

pub enum BundleFormat {
    Esm,  // ES modules (default)
    Cjs,  // CommonJS
}
```

## Integration Flow

### Current State: Path-Based Modules

```
User Code + Module Paths
         │
         ▼
┌────────────────────┐
│  JsExec::run_with  │
│     _modules()     │
└─────────┬──────────┘
          │
          ▼
┌────────────────────┐
│  Load modules from │
│  filesystem paths  │
└─────────┬──────────┘
          │
          ▼
┌────────────────────┐
│ Generate workerd   │
│ config with modules│
└─────────┬──────────┘
          │
          ▼
┌────────────────────┐
│ Spawn workerd with │
│ embedded user code │
└────────────────────┘
```

### Future State: With NPM Bundling

```
User Code + NPM Packages
         │
         ▼
┌────────────────────┐
│  JsExec::execute() │
└─────────┬──────────┘
          │
          ▼
┌────────────────────┐
│  Check cache for   │
│  bundled packages  │
└─────┬───────┬──────┘
      │       │
   Hit│       │Miss
      │       ▼
      │  ┌────────────────┐
      │  │ Bundle package │
      │  │ with esbuild   │
      │  └───────┬────────┘
      │          │
      │          ▼
      │  ┌────────────────┐
      │  │ Store in cache │
      │  └───────┬────────┘
      │          │
      └──────────┘
          │
          ▼
┌────────────────────┐
│ Add bundled code   │
│ as inline modules  │
└─────────┬──────────┘
          │
          ▼
┌────────────────────┐
│ Generate workerd   │
│ config with modules│
└─────────┬──────────┘
          │
          ▼
┌────────────────────┐
│ Spawn workerd with │
│ embedded user code │
└────────────────────┘
```

## Testing Strategy

### Path-Based Module Loading

**Unit Tests** (`integration.rs:test_execute_with_modules`):
- Create temporary module file
- Load via `run_with_modules()`
- Verify module functions are callable
- Test dash→underscore conversion

**Integration Tests** (`test_node_modules.rs`):
- Test with real node_modules (substrate-sandbox-ts)
- Verify dynamic imports work with node_modules symlink
- Test TypeScript transpilation as real-world example

### Bundling System

**Unit Tests** (`bundler.rs::tests`):
- Test `BundleConfig::default()`
- Test `NoCache` implementation
- Test `MockCache` get/put operations
- Verify cache key generation

**Integration Tests** (TODO):
- Bundle real packages (lodash, typescript)
- Test cache hit/miss paths
- Verify bundled code executes correctly in workerd
- Test error cases (missing package, esbuild failure)

## Error Handling

### BundleError Types

```rust
pub enum BundleError {
    /// IO error during bundling
    Io(String),

    /// esbuild process failed
    EsbuildFailed(String),

    /// Package not found in node_modules
    PackageNotFound(String),
}
```

### Error Recovery

- **Missing esbuild**: Return `BundleError::EsbuildFailed` with instructions
- **Package not found**: Return `BundleError::PackageNotFound` with package name
- **Cache failures**: Log but don't fail execution (fall back to bundling)

## Performance Considerations

### Caching Impact

Without cache:
- Cold start: ~100-500ms per package (esbuild time)
- Multiple packages compound linearly

With cache:
- Warm start: <10ms (cache lookup)
- Only bundle once per package version

### Bundle Size

Typical bundles:
- `lodash`: ~70KB minified
- `typescript`: ~5MB (includes full compiler)
- Most packages: 10-500KB range

Workerd loads these as inline strings, so large bundles increase config size but don't impact runtime performance.

### Memory Usage

- **Bundle cache**: Proportional to number of unique packages × bundle size
- **In-memory cache**: Simple but not shared across processes
- **Filesystem cache**: Minimal memory, shared across processes
- **Redis cache**: Minimal memory, shared across machines

## API Examples

### Path-Based Module Loading

```rust
use jsexec::{JsExec, JsExecConfig};
use futures::StreamExt;

let jsexec = JsExec::new(JsExecConfig::default());

let code = r#"
    import { helper } from 'my_module';
    return helper();
"#;

let mut stream = jsexec.run_with_modules(
    code.to_string(),
    vec!["./scripts/my-module.js".to_string()]
);

while let Some(event) = stream.next().await {
    println!("{:?}", event);
}
```

### NPM Package Bundling

```rust
use jsexec::{JsExec, JsExecConfig};
use std::path::PathBuf;

// Configure with node_modules path and packages to bundle
let config = JsExecConfig {
    node_modules_path: Some(PathBuf::from("./node_modules")),
    npm_packages: vec!["typescript".to_string()],
    esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
    ..Default::default()
};

let jsexec = JsExec::new(config);

// TypeScript is automatically bundled and available
// Both static and dynamic imports work!
let code = r#"
    import * as ts from 'typescript';

    const result = ts.transpile('const x: number = 42;');
    return result;
"#;

let stream = jsexec.run(code.to_string());
```

**Static Import Support**

JsExec automatically transforms static ES6 imports to dynamic imports under the hood:

```javascript
// You write (natural JavaScript):
import _ from 'lodash';
import * as ts from 'typescript';
import { sum, map } from 'lodash';

// Automatically transformed to:
const lodash_module = await import('lodash');
const _ = lodash_module.default || lodash_module;
const ts = await import('typescript');
const lodash_module = await import('lodash');
const { sum, map } = lodash_module;
```

This transformation happens transparently, so users can write natural JavaScript without worrying about the async function wrapper.

### Local Library Bundling (TypeScript/JavaScript)

Load your own TypeScript or JavaScript libraries:

```rust
use jsexec::{JsExec, JsExecConfig, LocalLibrary};
use std::path::PathBuf;

let config = JsExecConfig {
    local_libraries: vec![
        LocalLibrary {
            name: "my_utils".to_string(),
            entry_point: PathBuf::from("./lib/index.ts"),
            typecheck: true,  // Run tsc --noEmit before bundling
        }
    ],
    esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
    tsc_path: Some(PathBuf::from("./node_modules/.bin/tsc")),
    ..Default::default()
};

let jsexec = JsExec::new(config);

// Use your library with static imports
let code = r#"
    import { helper1, MyClass } from 'my_utils';

    const result = helper1(42);
    return result;
"#;

jsexec.run(code.to_string());
```

**How It Works:**

1. **Content Hashing** - Hash the entry point file content
2. **Cache Check** - Look for cached bundle with matching hash
3. **Type Checking** (if enabled) - Run `tsc --noEmit` to validate types
4. **Bundling** - esbuild compiles TypeScript and bundles all imports
5. **Caching** - Store bundled result keyed by content hash
6. **Subsequent Runs** - If file unchanged, use cached bundle (skip typecheck & bundling)

**Cache Invalidation:**
- Automatic - cache key includes file content hash
- When you edit the library, hash changes → cache miss → rebuild

**Combine npm + Local Libraries:**

```rust
let config = JsExecConfig {
    npm_packages: vec!["lodash".to_string()],
    local_libraries: vec![
        LocalLibrary {
            name: "my_helpers".to_string(),
            entry_point: PathBuf::from("./src/helpers.ts"),
            typecheck: true,
        }
    ],
    node_modules_path: Some(PathBuf::from("./node_modules")),
    esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
    ..Default::default()
};

// Use both in your code
let code = r#"
    import _ from 'lodash';
    import { format } from 'my_helpers';

    const data = [1, 2, 3];
    const sum = _.sum(data);
    return format(sum);
"#;
```

## Future Work

### Completed

1. ✅ **Bundler integration** - npm packages are automatically bundled when configured in `JsExecConfig.npm_packages`
2. ✅ **Dynamic import support** - bundled packages work with `await import()` syntax
3. ✅ **Static import transformation** - static ES6 imports automatically converted to dynamic imports
4. ✅ **Combined with path modules** - can use both npm packages and path-based modules together
5. ✅ **Local library bundling** - Bundle TypeScript/JavaScript libraries with type checking and caching
6. ✅ **Content-based caching** - Automatic cache invalidation based on file content hash

### Immediate TODOs

1. **Implement production cache**
   - Filesystem cache with version hashing
   - Proper error handling and logging
   - Cache invalidation strategy

3. **Version awareness**
   - Include package version in cache key
   - Detect package.json changes
   - Support version pinning

### Long-Term Enhancements

1. **Bundling optimizations**
   - Tree-shaking for smaller bundles
   - Code splitting for large packages
   - Parallel bundling for multiple packages

2. **Dynamic imports**
   - Support `await import()` for lazy loading
   - Bundle on first import, not at startup
   - Per-execution module sets

3. **Source maps**
   - Generate source maps during bundling
   - Map errors back to original npm package code
   - Better debugging experience

4. **CDN integration**
   - Fetch pre-bundled packages from CDN (esm.sh, skypack)
   - Fall back to local bundling if needed
   - Reduce esbuild dependency

## Security Considerations

### Path-Based Modules

- **Path traversal**: Validate paths don't escape intended directories
- **Arbitrary code execution**: Modules run in same sandbox as user code (already isolated)
- **File access**: Only read access needed for module files

### Bundling

- **Command injection**: Sanitize package names before passing to esbuild
- **Malicious packages**: Same risk as using npm directly (user responsibility)
- **Cache poisoning**: Validate cache keys, consider cryptographic hashing

### Recommendations

1. **Validate package names**: Use regex to ensure valid npm package format
2. **Sandbox esbuild**: Consider running esbuild in isolated process/container
3. **Audit dependencies**: Regular security audits of bundled packages
4. **Rate limiting**: Prevent DoS via excessive bundling requests

## Conclusion

This architecture provides:

✅ **Flexible module loading** via file paths with automatic name derivation
✅ **NPM package support** through on-demand bundling with esbuild
✅ **Extensible caching** via trait-based abstraction
✅ **Production-ready design** with error handling and testing strategy

The system balances ease of use (simple API), performance (caching), and flexibility (bring-your-own-cache) while working within workerd's constraints.
