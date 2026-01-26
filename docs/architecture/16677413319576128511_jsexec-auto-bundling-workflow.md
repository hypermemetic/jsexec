# JsExec Auto-Bundling Workflow - From Manual Configuration to Zero-Config

## Current State: What We've Built

### Module Loading System (Complete)

JsExec currently supports three types of module loading:

#### 1. Path-Based Modules
```rust
jsexec.run_with_modules(code, vec![
    "./helpers.js".to_string(),
    "./utils.js".to_string(),
]);
```
- Loads individual JavaScript files by path
- Module names derived from filenames (`helpers.js` → `helpers`)
- Dashes converted to underscores (`my-lib.js` → `my_lib`)

#### 2. NPM Package Bundling
```rust
let config = JsExecConfig {
    npm_packages: vec!["lodash".to_string(), "typescript".to_string()],
    node_modules_path: Some(PathBuf::from("./node_modules")),
    esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
    ..Default::default()
};
```
- Auto-bundles npm packages from node_modules
- Uses esbuild for bundling
- Packages become importable modules

#### 3. Local Library Bundling (TypeScript/JavaScript)
```rust
let config = JsExecConfig {
    local_libraries: vec![
        LocalLibrary {
            name: "plexus_client".to_string(),
            entry_point: PathBuf::from("../substrate-sandbox-ts/lib/index.ts"),
            typecheck: true,  // Optional type checking with tsc
        }
    ],
    esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
    tsc_path: Some(PathBuf::from("./node_modules/.bin/tsc")),
    ..Default::default()
};
```

**Features:**
- ✅ TypeScript support (esbuild compiles automatically)
- ✅ Optional type checking with tsc
- ✅ Content-based caching (hash of entry point)
- ✅ Auto-invalidation when files change
- ✅ Bundles entire dependency tree

#### 4. Static Import Transformation
```javascript
// User writes:
import _ from 'lodash';
import * as ts from 'typescript';
import { helper } from 'my_lib';

// Auto-transformed to:
const _module__mod = await import('lodash');
const _ = _module__mod.default || _module__mod;
const ts = await import('typescript');
const { helper } = await import('my_lib');
```

All static ES6 imports automatically converted to dynamic imports to work inside async function wrapper.

### What Works Today

**Complete workflow:**
```rust
use jsexec::{JsExec, JsExecConfig, LocalLibrary};

// Configure with libraries
let config = JsExecConfig {
    local_libraries: vec![
        LocalLibrary {
            name: "plexus_client".to_string(),
            entry_point: PathBuf::from("../substrate-sandbox-ts/lib/index.ts"),
            typecheck: false,
        },
        LocalLibrary {
            name: "agent".to_string(),
            entry_point: PathBuf::from("../substrate-sandbox-ts/src/agent/index.ts"),
            typecheck: false,
        },
    ],
    esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
    ..Default::default()
};

let jsexec = JsExec::new(config);

// Write code that imports the libraries
let code = r#"
    import { createClient } from 'plexus_client';
    import { createAgent } from 'agent';

    const rpc = createClient({ url: 'ws://localhost:4444' });
    const agent = createAgent('my-agent');
    // ... use the libraries
"#;

jsexec.run(code.to_string());
```

**Test Results:**
- ✅ substrate-sandbox-ts library loads (70ms)
- ✅ Agent libraries load and execute
- ✅ Static imports work seamlessly
- ✅ Multiple libraries can be combined
- ✅ npm packages + local libraries work together

## The Problem: Manual Configuration

### Current User Experience

To run a script, users must:

1. **Manually identify dependencies**
   ```typescript
   // Looking at: src/agent-simple-example.ts
   import { createClient } from '@plexus/client/transport';
   import { Cone } from '@plexus/client';
   import { createAgent } from './agent';
   import { createSynapseCallTool } from './agent/tools-simple';
   ```
   User thinks: "I need plexus client and agent libraries"

2. **Figure out entry points**
   - Where is `@plexus/client`? → Check package.json → `lib/index.ts`
   - Where is `./agent`? → Check relative paths → `src/agent/index.ts`

3. **Configure JsExec manually**
   ```rust
   let config = JsExecConfig {
       local_libraries: vec![
           LocalLibrary {
               name: "plexus_client".to_string(),
               entry_point: PathBuf::from("../substrate-sandbox-ts/lib/index.ts"),
               typecheck: false,
           },
           LocalLibrary {
               name: "agent".to_string(),
               entry_point: PathBuf::from("../substrate-sandbox-ts/src/agent/index.ts"),
               typecheck: false,
           },
       ],
       esbuild_path: Some(PathBuf::from("./node_modules/.bin/esbuild")),
       ..Default::default()
   };
   ```

4. **Rewrite import paths in code**
   ```javascript
   // Original:
   import { createClient } from '@plexus/client/transport';

   // Rewritten:
   import { createClient } from 'plexus_client';
   ```

This is **tedious and error-prone**.

### What Users Actually Want

```rust
// Just run the damn file!
jsexec.run_file("../substrate-sandbox-ts/src/agent-simple-example.ts");
```

That's it. No configuration. No manual dependency tracking. No import rewriting.

## The Solution: Auto-Bundling Workflow

### Key Insight: esbuild Already Does This!

When you point esbuild at a file, it **automatically**:
- ✅ Resolves all imports (npm packages, relative paths, aliases)
- ✅ Follows the entire dependency tree recursively
- ✅ Handles TypeScript compilation
- ✅ Applies tsconfig.json path mappings
- ✅ Bundles everything into one self-contained file

**Example:**
```bash
esbuild src/agent-simple-example.ts --bundle --format=esm
```

Output: Single JavaScript file with:
- All imports resolved
- All dependencies included
- TypeScript compiled
- Ready to execute

### Proposed API

```rust
impl JsExec {
    /// Run a JavaScript/TypeScript file with automatic dependency bundling
    ///
    /// This method:
    /// 1. Detects if the file is TypeScript or JavaScript
    /// 2. Bundles the file with esbuild (resolving ALL imports)
    /// 3. Optionally type-checks with tsc
    /// 4. Caches the bundled result (keyed by file content hash)
    /// 5. Executes the bundled code
    ///
    /// # Example
    /// ```rust
    /// let jsexec = JsExec::new(JsExecConfig::default());
    ///
    /// // Just run the file - all dependencies auto-bundled
    /// jsexec.run_file("./src/my-script.ts");
    /// ```
    pub fn run_file(
        &self,
        path: impl AsRef<Path>
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static;

    /// Run a file with type checking enabled
    pub fn run_file_checked(
        &self,
        path: impl AsRef<Path>
    ) -> impl Stream<Item = JsExecEvent> + Send + 'static;
}
```

### How It Works

#### Step 1: Detect File Type
```rust
let path = path.as_ref();
let is_typescript = path.extension()
    .and_then(|e| e.to_str())
    .map(|e| e == "ts" || e == "tsx")
    .unwrap_or(false);
```

#### Step 2: Compute Cache Key
```rust
// Hash file content for cache invalidation
let content = tokio::fs::read_to_string(path).await?;
let hash = compute_md5_hash(&content);
let cache_key = format!("file_{}_{}", path.display(), hash);
```

#### Step 3: Check Cache
```rust
if let Some(cached_bundle) = cache.get(&cache_key).await {
    return Ok(cached_bundle);
}
```

#### Step 4: Type Check (Optional)
```rust
if typecheck && is_typescript {
    typecheck_file(path, tsc_path).await?;
}
```

#### Step 5: Bundle with esbuild
```rust
let output = Command::new(esbuild_path)
    .arg(path)
    .arg("--bundle")
    .arg("--format=esm")
    .arg("--platform=browser")
    .arg("--minify")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()
    .await?;

let bundled_code = String::from_utf8(output.stdout)?;
```

**esbuild automatically handles:**
- All imports (`import { X } from 'Y'`)
- Relative paths (`./agent`, `../lib/utils`)
- npm packages (`lodash`, `typescript`)
- TypeScript aliases (`@plexus/client` → via tsconfig.json)
- Nested dependencies (transitive imports)

#### Step 6: Cache Result
```rust
cache.put(&cache_key, bundled_code.clone()).await;
```

#### Step 7: Execute
```rust
runner::execute(bundled_code, context, limits, runner_config)
```

### Workflow Comparison

#### Before (Manual Configuration)
```
User wants to run: agent-simple-example.ts

1. Analyze imports manually
   - @plexus/client → lib/index.ts
   - ./agent → src/agent/index.ts
   - ./agent/tools-simple → src/agent/tools-simple.ts

2. Configure JsExec
   - Add each library
   - Specify entry points
   - Set paths

3. Rewrite imports in code
   - @plexus/client → plexus_client
   - ./agent → agent

4. Run code
```

#### After (Auto-Bundling)
```
User wants to run: agent-simple-example.ts

1. jsexec.run_file("agent-simple-example.ts")

Done.
```

### Cache Behavior

**First run:**
```
1. Hash agent-simple-example.ts → "abc123"
2. Cache miss for "file_agent-simple-example.ts_abc123"
3. Bundle with esbuild (~500ms for large projects)
4. Cache bundled result
5. Execute (70ms)

Total: ~570ms
```

**Second run (no changes):**
```
1. Hash agent-simple-example.ts → "abc123" (same)
2. Cache HIT
3. Execute cached bundle (70ms)

Total: ~70ms (8x faster!)
```

**After editing the file:**
```
1. Hash agent-simple-example.ts → "def456" (changed)
2. Cache miss
3. Re-bundle and cache
4. Execute

Total: ~570ms (auto-invalidated)
```

## Technical Design

### Architecture

```
User Code File (agent-simple-example.ts)
        │
        ▼
    run_file()
        │
        ├─► Compute content hash
        │
        ├─► Check cache
        │   │
        │   ├─► HIT → return cached bundle
        │   │
        │   └─► MISS:
        │       │
        │       ├─► Type check (optional)
        │       │   └─► tsc --noEmit file.ts
        │       │
        │       ├─► Bundle with esbuild
        │       │   └─► esbuild file.ts --bundle
        │       │       │
        │       │       ├─► Resolves all imports
        │       │       ├─► Follows dependency tree
        │       │       ├─► Compiles TypeScript
        │       │       ├─► Bundles everything
        │       │       └─► Output: single JS file
        │       │
        │       └─► Cache result
        │
        └─► Execute bundled code
            └─► runner::execute()
```

### Cache Key Strategy

**Format:** `file_{sanitized_path}_{content_hash}`

**Why this works:**
- Different files → different keys (even with same content)
- Same file, different content → different keys (auto-invalidation)
- Same file, same content → same key (cache hit)

**Example:**
```
File: src/agent-simple-example.ts
Content hash: 7fa6a8d3c2b1...
Key: file_src_agent-simple-example.ts_7fa6a8d3c2b1...
```

### Error Handling

```rust
pub enum RunFileError {
    /// File not found or not readable
    FileNotFound(PathBuf),

    /// Type checking failed (tsc errors)
    TypeCheckFailed(String),

    /// Bundling failed (esbuild errors)
    BundleFailed(String),

    /// IO error
    Io(String),
}
```

**Recovery strategies:**
- File not found → Clear error message with path
- Type check failed → Show tsc output, suggest `run_file()` without checking
- Bundle failed → Show esbuild errors (missing modules, syntax errors)

### Configuration Options

```rust
pub struct RunFileConfig {
    /// Whether to type check before bundling
    pub typecheck: bool,

    /// Path to esbuild binary (defaults to "esbuild" in PATH)
    pub esbuild_path: Option<PathBuf>,

    /// Path to tsc binary (defaults to "tsc" in PATH)
    pub tsc_path: Option<PathBuf>,

    /// Whether to use cache (defaults to true)
    pub use_cache: bool,

    /// Working directory for resolving relative imports
    /// (defaults to parent directory of the file)
    pub working_dir: Option<PathBuf>,
}

impl Default for RunFileConfig {
    fn default() -> Self {
        Self {
            typecheck: false,
            esbuild_path: None,
            tsc_path: None,
            use_cache: true,
            working_dir: None,
        }
    }
}
```

### API Variants

```rust
impl JsExec {
    /// Run file with default config (no type checking, with cache)
    pub fn run_file(&self, path: impl AsRef<Path>)
        -> impl Stream<Item = JsExecEvent>;

    /// Run file with type checking
    pub fn run_file_checked(&self, path: impl AsRef<Path>)
        -> impl Stream<Item = JsExecEvent>;

    /// Run file with custom config
    pub fn run_file_with_config(
        &self,
        path: impl AsRef<Path>,
        config: RunFileConfig
    ) -> impl Stream<Item = JsExecEvent>;
}
```

## Benefits

### 1. Zero Configuration
```rust
// Before: ~50 lines of config
let config = JsExecConfig {
    local_libraries: vec![/* ... */],
    npm_packages: vec![/* ... */],
    esbuild_path: Some(/* ... */),
    // ...
};

// After: 1 line
jsexec.run_file("script.ts");
```

### 2. Works With Any Project Structure

```
project/
├── src/
│   ├── main.ts                 ← jsexec.run_file("src/main.ts")
│   ├── utils/
│   │   └── helpers.ts          ← imported automatically
│   └── lib/
│       └── api.ts              ← imported automatically
├── node_modules/
│   └── lodash/                 ← bundled automatically
└── tsconfig.json               ← aliases resolved automatically
```

No manual mapping needed!

### 3. Automatic Dependency Resolution

```typescript
// Your script:
import _ from 'lodash';                    // ← npm package
import { createClient } from '@plexus/client';  // ← tsconfig alias
import { helper } from './utils/helper';        // ← relative import
import { API } from '../lib/api';               // ← parent directory

// esbuild finds and bundles ALL of these automatically!
```

### 4. Development Workflow

**Edit-Run loop:**
```bash
# Edit your TypeScript file
vim src/agent.ts

# Run it
cargo run -- run-file src/agent.ts

# Edit again
vim src/agent.ts

# Run again (cache auto-invalidates)
cargo run -- run-file src/agent.ts
```

No build step. No manual bundling. Just edit and run.

### 5. Works With Existing TypeScript Projects

Any existing TypeScript project can be run immediately:

```rust
// Run a Next.js component
jsexec.run_file("components/MyComponent.tsx");

// Run a Node.js script
jsexec.run_file("scripts/migrate-db.ts");

// Run a React app entry point
jsexec.run_file("src/index.tsx");
```

(Subject to workerd compatibility - DOM APIs, Node.js APIs, etc.)

## Implementation Plan

### Phase 1: Core run_file() Implementation
1. Add `run_file()` method to JsExec
2. Implement file hashing for cache keys
3. Wire up esbuild bundling
4. Connect to execution pipeline
5. Basic error handling

### Phase 2: Type Checking Integration
1. Add `run_file_checked()` variant
2. Integrate tsc type checking
3. Error message formatting
4. Skip type checking on cache hit

### Phase 3: Configuration & Customization
1. Add `RunFileConfig` struct
2. Implement `run_file_with_config()`
3. Support custom working directory
4. Support disabling cache

### Phase 4: Testing & Polish
1. Integration tests with real TypeScript projects
2. Test cache invalidation
3. Test error cases (missing files, type errors, bundle errors)
4. Documentation and examples

### Phase 5: CLI Integration
1. Add `jsexec run-file <path>` command
2. Add `--typecheck` flag
3. Add `--no-cache` flag
4. Add `--watch` mode for development

## Future Enhancements

### 1. Watch Mode
```rust
jsexec.watch_file("src/main.ts"); // Auto-reload on changes
```

### 2. Multi-File Projects
```rust
jsexec.run_project("./"); // Auto-detect entry point (package.json main field)
```

### 3. Dependency Analysis
```rust
let deps = jsexec.analyze_dependencies("src/main.ts");
// Returns: ["lodash", "@plexus/client", "./utils/helper"]
```

### 4. Source Maps
```rust
// Generate source maps for debugging
jsexec.run_file_with_sourcemap("src/main.ts");
```

### 5. Hot Module Replacement
```rust
// For long-running processes
jsexec.run_with_hmr("src/server.ts");
```

## Comparison With Alternatives

### vs. Traditional Build Step
```bash
# Traditional: 2 steps
tsc src/main.ts      # Compile
node dist/main.js    # Run

# JsExec: 1 step
jsexec run-file src/main.ts
```

### vs. ts-node
```bash
# ts-node: Limited to Node.js APIs
ts-node src/main.ts

# JsExec: V8 sandbox, Plexus access, browser APIs
jsexec run-file src/main.ts
```

### vs. Deno
```bash
# Deno: Good for TypeScript, but different runtime
deno run src/main.ts

# JsExec: TypeScript + Plexus + workerd
jsexec run-file src/main.ts
```

### vs. Manual JsExec Configuration
```rust
// Manual: ~50 lines
let config = JsExecConfig { /* ... */ };
let jsexec = JsExec::new(config);
jsexec.run(code);

// Auto: 1 line
jsexec.run_file("script.ts");
```

## Conclusion

The auto-bundling workflow transforms JsExec from a powerful-but-complex tool into a **simple, zero-config JavaScript/TypeScript runner**.

**Before:** Manual configuration, dependency tracking, import rewriting
**After:** `jsexec.run_file("script.ts")` - done

By leveraging esbuild's existing dependency resolution, we get:
- ✅ Zero configuration
- ✅ Automatic dependency bundling
- ✅ TypeScript compilation
- ✅ Content-based caching
- ✅ Works with any project structure

This makes JsExec suitable for:
- Quick script execution
- Testing TypeScript libraries
- Running agent workflows
- Development iteration (edit-run loop)
- CI/CD pipelines (no build step needed)

The implementation reuses our existing bundling infrastructure (esbuild, tsc, caching) and simply applies it at a higher level - the file level instead of the library level.
