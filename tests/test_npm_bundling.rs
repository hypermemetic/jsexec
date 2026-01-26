//! Test NPM package bundling

use std::pin::pin;
use futures_util::StreamExt;
use jsexec::{JsExec, JsExecConfig, JsExecEvent};

/// Check if workerd is available
fn workerd_available() -> bool {
    std::process::Command::new("workerd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if esbuild is available
fn esbuild_available() -> bool {
    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        return std::process::Command::new("esbuild")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    }

    std::process::Command::new(&esbuild_path)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Helper macro to skip tests if dependencies not available
macro_rules! require_deps {
    () => {
        if !workerd_available() {
            eprintln!("Skipping test: workerd not available");
            return;
        }
        if !esbuild_available() {
            eprintln!("Skipping test: esbuild not available");
            return;
        }
    };
}

#[tokio::test]
async fn test_bundle_and_use_typescript() {
    require_deps!();

    // Path to substrate-sandbox-ts node_modules
    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found at {}", node_modules_path.display());
        return;
    }

    // Configure jsexec with npm packages
    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec!["typescript".to_string()],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code that uses the bundled TypeScript package
    let code = r#"
        // TypeScript should be available as a bundled module
        const ts = await import('typescript');

        // Test that TypeScript is loaded
        const tsModule = ts.default || ts;
        const hasTranspile = typeof tsModule.transpile === 'function';

        // Try transpiling some code
        let output = '';
        if (hasTranspile) {
            const tsCode = 'const x: number = 42;';
            output = tsModule.transpile(tsCode);
        }

        return {
            hasTypescript: typeof ts !== 'undefined',
            hasTranspile,
            outputLength: output.length,
            hasOutput: output.length > 0
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());

                assert_eq!(value["hasTypescript"], true, "TypeScript should be loaded");
                assert_eq!(value["hasTranspile"], true, "TypeScript.transpile should be available");
                assert_eq!(value["hasOutput"], true, "Should have transpiled output");

                found_result = true;
            }
            JsExecEvent::Error { message, name, stack, .. } => {
                eprintln!("Execution error ({}): {}", name, message);
                for line in stack {
                    eprintln!("  {:?}", line);
                }
                panic!("Execution error ({}): {}", name, message);
            }
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            _ => {
                println!("Event: {:?}", event);
            }
        }
    }

    assert!(found_result, "Should have received a result");
}

#[tokio::test]
async fn test_bundle_multiple_packages() {
    require_deps!();

    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found");
        return;
    }

    // Configure with multiple npm packages
    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec![
            "typescript".to_string(),
            // Add more packages here if available
        ],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    let code = r#"
        const ts = await import('typescript');
        const tsModule = ts.default || ts;

        return {
            typescriptAvailable: typeof ts !== 'undefined',
            version: tsModule.version || 'unknown'
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["typescriptAvailable"], true);
                found_result = true;
            }
            JsExecEvent::Error { message, name, .. } => {
                panic!("Execution error ({}): {}", name, message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a result");
}

#[tokio::test]
async fn test_bundling_with_path_modules() {
    require_deps!();

    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found");
        return;
    }

    // Create a temporary helper module
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let helper_path = temp_dir.path().join("helper.js");
    std::fs::write(&helper_path, "export function greet() { return 'Hello'; }")
        .expect("Failed to write helper module");

    // Configure with npm packages
    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec!["typescript".to_string()],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code that uses both bundled npm package and path-based module
    let code = r#"
        const ts = await import('typescript');
        const helper = await import('helper');

        const tsModule = ts.default || ts;
        const greet = helper.greet;

        return {
            typescriptWorks: typeof ts !== 'undefined',
            helperWorks: greet() === 'Hello',
            combined: greet() + ' from TypeScript ' + (tsModule.version || 'unknown')
        };
    "#;

    let helper_path_str = helper_path.to_str().unwrap().to_string();
    let mut stream = pin!(jsexec.run_with_modules(code.to_string(), vec![helper_path_str]));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["typescriptWorks"], true, "TypeScript should work");
                assert_eq!(value["helperWorks"], true, "Helper module should work");
                found_result = true;
            }
            JsExecEvent::Error { message, name, .. } => {
                panic!("Execution error ({}): {}", name, message);
            }
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a result");
}
