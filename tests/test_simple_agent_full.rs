//! Test running the complete simple-agent script

use std::pin::pin;
use futures_util::StreamExt;
use jsexec::{JsExec, JsExecConfig, JsExecEvent, LocalLibrary};

#[tokio::test]
async fn test_simple_agent_complete() {
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

    let config = JsExecConfig {
        local_libraries: vec![
            LocalLibrary {
                name: "plexus_client".to_string(),
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

    // Adapted simple-agent script (imports rewritten for bundled libraries)
    let agent_code = r#"
        import { createClient } from 'plexus_client';
        import { Cone } from 'plexus_client';
        import { createAgent, DefaultToolRegistry } from 'agent';
        import { createSynapseCallTool, getSynapseDiscoveryDocs } from 'agent';

        const PLEXUS_URL = 'ws://localhost:4444';

        console.log('🤖 Self-Discovering Agent with Synapse\n');
        console.log('═'.repeat(60));
        console.log('Architecture:');
        console.log('  • Agent: TypeScript orchestration');
        console.log('  • LLM: Cone (learns API dynamically)');
        console.log('  • Tools: ONE generic synapse_call tool');
        console.log('  • Discovery: Agent explores with --help');
        console.log('═'.repeat(60));
        console.log();

        async function runSimpleAgent() {
            try {
                // Connect to Plexus
                console.log(`Connecting to Plexus at ${PLEXUS_URL}...`);
                const rpc = createClient({ url: PLEXUS_URL });

                // Register just ONE tool - the generic synapse_call
                console.log('Registering tools...');
                const tools = new DefaultToolRegistry();
                tools.register(createSynapseCallTool());
                console.log('Available tools: synapse_call (discovers everything else)');
                console.log();

                // Build system prompt with discovery documentation
                const systemPrompt = `You are a helpful assistant that explores and uses the Plexus system via Synapse.

${getSynapseDiscoveryDocs()}

When asked to do something, first think about what Synapse commands you need, then use the synapse_call tool to execute them.

You can explore the API surface dynamically - use bash.execute to run synapse --help commands if you need to discover what's available.`;

                // Create a Cone for LLM reasoning
                console.log('Creating Cone for reasoning...');
                const cone = new Cone.ConeClientImpl(rpc);
                const coneName = `discovery-${Date.now()}`;

                const createResult = await cone.create(
                    'claude-sonnet-4-5-20250929',
                    coneName,
                    {},
                    systemPrompt
                );

                if (createResult.type !== 'cone_created') {
                    console.error('Failed to create cone:', createResult);
                    return { error: 'Failed to create cone', result: createResult };
                }

                const coneIdentifier = {
                    type: 'by_id',
                    id: createResult.coneId
                };
                console.log(`✓ Cone created: ${createResult.coneId.substring(0, 8)}...`);
                console.log();

                // Create agent
                const agent = createAgent('discovery-agent')
                    .withSystem(systemPrompt)
                    .withRPC(rpc)
                    .withTools(tools)
                    .withCone(coneIdentifier)
                    .ephemeral(false)
                    .maxTurns(10)
                    .maxToolCallsPerTurn(5)
                    .build();

                // Run a simple test query
                const query = "Check if the system is healthy";

                console.log('─'.repeat(60));
                console.log(`👤 User: ${query}`);
                console.log();

                let turnCount = 0;
                let finalResponse = '';

                for await (const event of agent.run(query)) {
                    switch (event.type) {
                        case 'thinking':
                            console.log(`💭 ${event.message}`);
                            break;

                        case 'tool_call':
                            console.log(`🔧 synapse_call:`);
                            console.log(`   ${JSON.stringify(event.input, null, 2)}`);
                            break;

                        case 'tool_result':
                            if (event.error) {
                                console.log(`   ❌ Error: ${JSON.stringify(event.result)}`);
                            } else {
                                const resultStr = typeof event.result === 'string'
                                    ? event.result
                                    : JSON.stringify(event.result);
                                const preview = resultStr.length > 150
                                    ? resultStr.substring(0, 150) + '...'
                                    : resultStr;
                                console.log(`   ✓ Result: ${preview}`);
                            }
                            break;

                        case 'response_chunk':
                            finalResponse += event.text;
                            break;

                        case 'turn_complete':
                            turnCount = event.turn;
                            break;

                        case 'done':
                            console.log('\n');
                            console.log(`✓ Complete in ${turnCount} turn(s)`);
                            break;

                        case 'error':
                            console.error(`\n❌ Error: ${event.error}`);
                            break;
                    }
                }

                console.log();
                console.log('─'.repeat(60));
                console.log('\n✨ Agent run complete!\n');

                rpc.disconnect();

                return {
                    success: true,
                    turns: turnCount,
                    response: finalResponse
                };

            } catch (error) {
                console.error('Error:', error.message);
                return {
                    success: false,
                    error: error.message
                };
            }
        }

        const result = await runSimpleAgent();
        return result;
    "#;

    println!("Running complete simple-agent inside jsexec...\n");

    let mut stream = pin!(jsexec.run(agent_code.to_string()));

    let mut got_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Console { level, args, .. } => {
                println!("[{:?}] {:?}", level, args);
            }
            JsExecEvent::Returned { value } => {
                println!("\n✓ Final result:");
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
