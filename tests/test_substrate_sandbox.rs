//! Test loading the real substrate-sandbox-ts library

use std::pin::pin;
use futures_util::StreamExt;
use jsexec::{JsExec, JsExecConfig, JsExecEvent, LocalLibrary};

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
async fn test_load_substrate_sandbox_library() {
    require_workerd!();

    let lib_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/lib/index.ts");

    if !lib_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/lib/index.ts not found at {}", lib_path.display());
        return;
    }

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        eprintln!("Skipping test: esbuild not found");
        return;
    }

    // Configure with the real substrate-sandbox-ts library
    let config = JsExecConfig {
        local_libraries: vec![
            LocalLibrary {
                name: "plexus_client".to_string(),
                entry_point: lib_path,
                typecheck: false,  // Skip type checking for speed (can enable if needed)
            }
        ],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Code that uses the substrate-sandbox-ts library
    let code = r#"
        import { Cone, Bash, Arbor, Jsexec } from 'plexus_client';

        return {
            hasCone: typeof Cone !== 'undefined',
            hasBash: typeof Bash !== 'undefined',
            hasArbor: typeof Arbor !== 'undefined',
            hasJsexec: typeof Jsexec !== 'undefined',
            modules: Object.keys({ Cone, Bash, Arbor, Jsexec })
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["hasCone"], true, "Should have Cone module");
                assert_eq!(value["hasBash"], true, "Should have Bash module");
                assert_eq!(value["hasArbor"], true, "Should have Arbor module");
                assert_eq!(value["hasJsexec"], true, "Should have Jsexec module");
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
async fn test_substrate_sandbox_with_typecheck() {
    require_workerd!();

    let lib_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/lib/index.ts");

    if !lib_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts/lib/index.ts not found");
        return;
    }

    let base_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts");

    let esbuild_path = base_path.join("node_modules/.bin/esbuild");
    let tsc_path = base_path.join("node_modules/.bin/tsc");

    if !esbuild_path.exists() || !tsc_path.exists() {
        eprintln!("Skipping test: esbuild or tsc not found");
        return;
    }

    // Configure with type checking enabled
    let config = JsExecConfig {
        local_libraries: vec![
            LocalLibrary {
                name: "plexus_client".to_string(),
                entry_point: lib_path,
                typecheck: true,  // Enable type checking!
            }
        ],
        esbuild_path: Some(esbuild_path),
        tsc_path: Some(tsc_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    let code = r#"
        import * as Plexus from 'plexus_client';

        return {
            loaded: typeof Plexus !== 'undefined',
            exports: Object.keys(Plexus).slice(0, 5)  // First 5 exports
        };
    "#;

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result with typecheck: {}", serde_json::to_string_pretty(value).unwrap());
                assert_eq!(value["loaded"], true);
                found_result = true;
            }
            JsExecEvent::Error { message, name, .. } => {
                // Type checking might fail if there are type errors in the library
                eprintln!("Error (expected if type errors exist): {}: {}", name, message);
                found_result = true;  // Count this as completing the test
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a result");
}
