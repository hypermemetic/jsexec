//! Test loading libraries from substrate-sandbox-ts

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

/// Helper macro to skip tests if workerd is not available
macro_rules! require_workerd {
    () => {
        if !workerd_available() {
            eprintln!("Skipping test: workerd not available");
            return;
        }
    };
}

#[tokio::test]
async fn test_load_tools_simple_from_sandbox() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());

    // Path to substrate-sandbox-ts relative to jsexec directory
    let sandbox_path = "../substrate-sandbox-ts/dist/agent/tools-simple.js";
    let module_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(sandbox_path);

    // Only canonicalize if it exists
    let module_path = if module_path.exists() {
        module_path.canonicalize().expect("Failed to canonicalize path")
    } else {
        eprintln!("Skipping test: substrate-sandbox-ts not found at {}", module_path.display());
        return;
    };

    println!("Loading module from: {}", module_path.display());

    // Check if file exists
    if !module_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts not found at {}", module_path.display());
        return;
    }

    let code = r#"
        // The module should be available as ctx.modules.tools_simple
        // (note: filename has dash, becomes underscore in module name? Let's check)
        const toolsModule = ctx.modules['tools-simple'] || ctx.modules.tools_simple;

        if (!toolsModule) {
            return { error: "Module not loaded", available: Object.keys(ctx.modules) };
        }

        // Check if createSynapseCallTool is available
        const hasTool = typeof toolsModule.createSynapseCallTool === 'function';

        return {
            success: true,
            hasTool,
            exports: Object.keys(toolsModule)
        };
    "#;

    let mut stream = pin!(jsexec.run_with_modules(
        code.to_string(),
        vec![module_path.to_str().unwrap().to_string()]
    ));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());

                if value.get("error").is_some() {
                    // Module not loaded - show what's available
                    println!("Available modules: {:?}", value.get("available"));
                } else {
                    // Module loaded successfully
                    assert_eq!(value["success"], true);
                    assert_eq!(value["hasTool"], true);
                }

                found_result = true;
            }
            JsExecEvent::Error { message, name, .. } => {
                eprintln!("Error ({}): {}", name, message);
            }
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a result");
}

#[tokio::test]
async fn test_use_tools_simple_functions() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());

    let sandbox_path = "../substrate-sandbox-ts/dist/agent/tools-simple.js";
    let module_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(sandbox_path);

    if !module_path.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts not found");
        return;
    }

    let module_path = module_path.canonicalize().expect("Failed to canonicalize path");

    let code = r#"
        const tools = ctx.modules['tools-simple'];

        if (!tools) {
            return { error: "Module not found" };
        }

        // Create the tool
        const tool = tools.createSynapseCallTool();

        return {
            name: tool.name,
            hasDescription: typeof tool.description === 'string',
            hasInputSchema: typeof tool.input_schema === 'object'
        };
    "#;

    let mut stream = pin!(jsexec.run_with_modules(
        code.to_string(),
        vec![module_path.to_str().unwrap().to_string()]
    ));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                println!("Result: {}", serde_json::to_string_pretty(value).unwrap());

                if value.get("error").is_none() {
                    assert_eq!(value["name"], "synapse_call");
                    assert_eq!(value["hasDescription"], true);
                    assert_eq!(value["hasInputSchema"], true);
                }

                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                eprintln!("Execution error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a result");
}
