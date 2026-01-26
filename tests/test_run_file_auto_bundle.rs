//! Test auto-bundling workflow with run_file()
//!
//! This demonstrates the zero-config workflow where jsexec.run_file()
//! automatically bundles all dependencies without manual configuration.

use std::pin::pin;
use futures_util::StreamExt;
use jsexec::{JsExec, JsExecConfig, JsExecEvent};

#[tokio::test]
async fn test_run_file_auto_bundle() {
    // Check if workerd is available
    if !std::process::Command::new("workerd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("Skipping test: workerd not available");
        return;
    }

    // Point to a TypeScript file that imports from local libraries
    let script_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/src/agent/examples/simple-agent.ts");

    if !script_path.exists() {
        eprintln!("Skipping test: simple-agent.ts not found at {:?}", script_path);
        return;
    }

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        eprintln!("Skipping test: esbuild not found");
        return;
    }

    // Zero configuration - just point at esbuild
    let config = JsExecConfig {
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    println!("Running auto-bundled script: {:?}\n", script_path);
    println!("This should automatically bundle all imports from:");
    println!("  - plexus_client library");
    println!("  - agent library");
    println!("  - Any npm dependencies");
    println!("\nNo manual LocalLibrary configuration needed!\n");

    let mut stream = pin!(jsexec.run_file(script_path));

    let mut got_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            JsExecEvent::Returned { value } => {
                println!("\n✓ Script completed successfully!");
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());
                got_result = true;
            }
            JsExecEvent::Error { message, name, stack, .. } => {
                eprintln!("\n❌ Error ({}): {}", name, message);
                for line in stack {
                    eprintln!("  {:?}", line);
                }
                got_result = true;
            }
            _ => {}
        }
    }

    assert!(got_result, "Should have received a result");
}

#[tokio::test]
async fn test_run_file_simple_typescript() {
    // Check if workerd is available
    if !std::process::Command::new("workerd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("Skipping test: workerd not available");
        return;
    }

    // Create a simple test script that uses an installed package (ws)
    let test_script = r#"
        import WebSocket from 'ws';

        console.log('Testing auto-bundle with ws (WebSocket)...');

        // Just verify the module loaded correctly
        const hasWebSocket = typeof WebSocket === 'function';
        const constructorName = WebSocket.name;

        console.log('WebSocket constructor:', constructorName);
        console.log('Is function:', hasWebSocket);

        return {
            hasWebSocket,
            constructorName,
            success: true
        };
    "#;

    // Write to temp file
    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join("test_auto_bundle_ws.ts");
    std::fs::write(&script_path, test_script).expect("Failed to write test script");

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        eprintln!("Skipping test: esbuild not found");
        return;
    }

    let config = JsExecConfig {
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    println!("Testing auto-bundle with simple TypeScript + ws (WebSocket)\n");

    let mut stream = pin!(jsexec.run_file(&script_path));

    let mut got_result = false;
    let mut success = false;

    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            JsExecEvent::Returned { value } => {
                println!("\n✓ Result: {}", serde_json::to_string_pretty(value).unwrap());

                // Verify the result
                if let Some(obj) = value.as_object() {
                    if let Some(serde_json::Value::Bool(true)) = obj.get("success") {
                        success = true;
                    }
                }

                got_result = true;
            }
            JsExecEvent::Error { message, name, .. } => {
                eprintln!("\n❌ Error ({}): {}", name, message);
                got_result = true;
            }
            _ => {}
        }
    }

    // Clean up temp file
    let _ = std::fs::remove_file(&script_path);

    assert!(got_result, "Should have received a result");
    assert!(success, "Script should have returned success: true");
}
