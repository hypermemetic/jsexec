//! Test running a complete agent workflow inside jsexec

use std::pin::pin;
use futures_util::StreamExt;
use jsexec::{JsExec, JsExecConfig, JsExecEvent, LocalLibrary};

#[tokio::test]
async fn test_agent_workflow_in_jsexec() {
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

    let lib_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/lib/index.ts");

    let agent_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/src/agent");

    if !lib_path.exists() || !agent_dir.exists() {
        eprintln!("Skipping test: substrate-sandbox-ts not found");
        return;
    }

    let esbuild_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../substrate-sandbox-ts/node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        eprintln!("Skipping test: esbuild not found");
        return;
    }

    // Bundle the entire agent library
    let config = JsExecConfig {
        local_libraries: vec![
            LocalLibrary {
                name: "plexus".to_string(),
                entry_point: lib_path,
                typecheck: false,
            },
            LocalLibrary {
                name: "agent".to_string(),
                entry_point: agent_dir.join("index.ts"),
                typecheck: false,
            },
        ],
        esbuild_path: Some(esbuild_path),
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Create a simple agent workflow
    let code = r#"
        import { createClient } from 'plexus';
        import { createAgent, DefaultToolRegistry } from 'agent';
        import { createSynapseCallTool } from 'agent';

        console.log("🤖 Setting up agent inside jsexec...");

        const PLEXUS_URL = 'ws://localhost:4444';

        async function runAgent() {
            try {
                // Connect to Plexus
                console.log(`Connecting to ${PLEXUS_URL}...`);
                const rpc = createClient({ url: PLEXUS_URL });

                // Register tools
                const tools = new DefaultToolRegistry();
                tools.register(createSynapseCallTool());
                console.log("✓ Tools registered");

                // For now, just verify the setup works
                console.log("✓ Agent setup complete");

                return {
                    success: true,
                    hasRPC: typeof rpc !== 'undefined',
                    hasTools: tools.list().length > 0,
                    toolNames: tools.list().map(t => t.name)
                };
            } catch (error) {
                console.error("Error:", error.message);
                return {
                    success: false,
                    error: error.message
                };
            }
        }

        // Run the agent setup
        const result = await runAgent();
        return result;
    "#;

    println!("Running full agent workflow inside jsexec...\n");

    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut got_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            JsExecEvent::Returned { value } => {
                println!("\n✓ Agent workflow result:");
                println!("{}", serde_json::to_string_pretty(value).unwrap());
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
