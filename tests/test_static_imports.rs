//! Test static import transformation with npm packages

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

/// Helper macro to skip tests if dependencies not available
macro_rules! require_deps {
    () => {
        if !workerd_available() {
            eprintln!("Skipping test: workerd not available");
            return;
        }
    };
}

#[tokio::test]
async fn test_static_import_default() {
    require_deps!();

    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found");
        return;
    }

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec!["typescript".to_string()],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code using STATIC imports (should be transformed automatically)
    let code = r#"
        import ts from 'typescript';

        const hasTranspile = typeof ts.transpile === 'function';
        const tsCode = 'const x: number = 42;';
        const output = hasTranspile ? ts.transpile(tsCode) : '';

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
async fn test_static_import_namespace() {
    require_deps!();

    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found");
        return;
    }

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec!["typescript".to_string()],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code using NAMESPACE import (import * as)
    let code = r#"
        import * as ts from 'typescript';

        return {
            hasTypescript: typeof ts !== 'undefined',
            version: ts.version || 'unknown'
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["hasTypescript"], true, "TypeScript should be loaded");
                found_result = true;
            }
            JsExecEvent::Error { message, name, stack, .. } => {
                eprintln!("Execution error ({}): {}", name, message);
                for line in stack {
                    eprintln!("  {:?}", line);
                }
                panic!("Execution error ({}): {}", name, message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a result");
}

#[tokio::test]
async fn test_static_import_mixed() {
    require_deps!();

    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found");
        return;
    }

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec!["typescript".to_string()],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code with both default and namespace imports
    let code = r#"
        import ts from 'typescript';
        import * as ts2 from 'typescript';

        return {
            defaultWorks: typeof ts !== 'undefined',
            namespaceWorks: typeof ts2 !== 'undefined',
            defaultHasVersion: typeof ts.version !== 'undefined',
            namespaceHasVersion: typeof ts2.version !== 'undefined'
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["defaultWorks"], true);
                assert_eq!(value["namespaceWorks"], true);
                // Both should have access to the module
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
