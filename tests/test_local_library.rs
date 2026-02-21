//! Test local library bundling with TypeScript support

use std::pin::pin;
use futures_util::StreamExt;
use plexus_jsexec::{JsExec, JsExecConfig, JsExecEvent, LocalLibrary};

/// Check if workerd is available
fn workerd_available() -> bool {
    std::process::Command::new("workerd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Helper macro to skip tests if workerd not available
macro_rules! require_workerd {
    () => {
        if !workerd_available() {
            eprintln!("Skipping test: workerd not available");
            return;
        }
    };
}

#[tokio::test]
async fn test_bundle_local_typescript_library() {
    require_workerd!();

    // Create a temporary TypeScript library
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let lib_path = temp_dir.path().join("mylib.ts");

    std::fs::write(&lib_path, r#"
        export function add(a: number, b: number): number {
            return a + b;
        }

        export function multiply(a: number, b: number): number {
            return a * b;
        }

        export const PI: number = 3.14159;
    "#).expect("Failed to write library file");

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        eprintln!("Skipping test: esbuild not found at {}", esbuild_path.display());
        return;
    }

    // Configure with local library
    let config = JsExecConfig {
        local_libraries: vec![
            LocalLibrary {
                name: "mylib".to_string(),
                entry_point: lib_path.clone(),
                typecheck: false,  // Skip type checking for this simple test
            }
        ],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code that uses the bundled library
    let code = r#"
        import { add, multiply, PI } from 'mylib';

        const sum = add(5, 3);
        const product = multiply(4, 7);

        return {
            sum,
            product,
            pi: PI,
            works: sum === 8 && product === 28
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["sum"], 8);
                assert_eq!(value["product"], 28);
                assert_eq!(value["works"], true);
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
async fn test_local_library_with_npm_package() {
    require_workerd!();

    // Create a temporary library
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let lib_path = temp_dir.path().join("utils.ts");

    std::fs::write(&lib_path, r#"
        export function greet(name: string): string {
            return `Hello, ${name}!`;
        }
    "#).expect("Failed to write library file");

    let node_modules_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules");

    if !node_modules_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/node_modules not found");
        return;
    }

    let esbuild_path = node_modules_path.join(".bin/esbuild");

    // Configure with both npm package and local library
    let config = JsExecConfig {
        node_modules_path: Some(node_modules_path),
        npm_packages: vec!["typescript".to_string()],
        local_libraries: vec![
            LocalLibrary {
                name: "utils".to_string(),
                entry_point: lib_path,
                typecheck: false,
            }
        ],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code that uses both npm package and local library
    let code = r#"
        import * as ts from 'typescript';
        import { greet } from 'utils';

        const greeting = greet("World");
        const hasTypescript = typeof ts !== 'undefined';

        return {
            greeting,
            hasTypescript,
            combined: greeting + " TypeScript version: " + (ts.version || "unknown")
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["greeting"], "Hello, World!");
                assert_eq!(value["hasTypescript"], true);
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
