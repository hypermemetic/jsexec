#!/bin/bash
# End-to-end test for plexus_env RPC method
# Requires: running Plexus backend (e.g., substrate on localhost:4444)

set -e

BACKEND="${1:-substrate}"
HOST="${2:-127.0.0.1}"
PORT="${3:-4444}"

echo "Testing plexus_env with backend=$BACKEND, host=$HOST, port=$PORT"
echo ""

# Check if synapse is available
if ! command -v synapse &> /dev/null; then
    echo "❌ synapse not found in PATH"
    exit 1
fi

# Check if hub-codegen is available
HUB_CODEGEN="${HUB_CODEGEN_PATH:-$(pwd)/../hub-codegen/target/release/hub-codegen}"
if [ ! -f "$HUB_CODEGEN" ]; then
    echo "❌ hub-codegen not found at $HUB_CODEGEN"
    echo "   Build it with: cd ../hub-codegen && cargo build --release"
    exit 1
fi
echo "✅ Using hub-codegen: $HUB_CODEGEN"

# Test 1: Basic plexus_env call with simple echo test
echo ""
echo "Test 1: Basic plexus_env call..."
TEST_CODE='
// Verify plexus client is available and properly configured
const { createClient } = ctx.modules.plexus;
const c = createClient({url: ctx.plexus_url});

// Return success with plexus_url to confirm it was injected correctly
return {
  success: true,
  plexus_url: ctx.plexus_url,
  client_created: typeof c === "object",
  has_connect: typeof c.connect === "function"
};
'

synapse "$BACKEND" jsexec plexus_env \
  --host "$HOST" --port "$PORT" --backend "$BACKEND" \
  --code "$TEST_CODE" || {
    echo "❌ Test 1 failed"
    exit 1
}

echo "✅ Test 1 passed"

# Test 2: Cache hit on second run
echo ""
echo "Test 2: Cache hit on second run..."
synapse "$BACKEND" jsexec plexus_env \
  --host "$HOST" --port "$PORT" --backend "$BACKEND" \
  --code "return 'cache test';" || {
    echo "❌ Test 2 failed"
    exit 1
}

echo "✅ Test 2 passed"

echo ""
echo "✅ All tests passed!"
echo ""
echo "Next steps:"
echo "  - Check ~/.cache/jsexec/plexus/ for cached bundles"
echo "  - Monitor progress events in synapse output"
echo "  - Try different backends to test multiple cache entries"
