#!/bin/bash
# stress_test.sh — Launch 100 concurrent phi CLI invocations
#
# Validates that fs2 file-lock based session management handles
# process-level concurrency without panics or deadlocks.
#
# Prerequisites:
#   - phi binary built (cargo build --release)
#   - OPENAI_API_KEY or equivalent set in environment
#   - A small prompt that returns quickly
#
# Usage:
#   ./scripts/stress_test.sh [concurrency] [prompt]
#   Default: 50 concurrent processes, prompt "echo ok"

set -euo pipefail

CONCURRENCY="${1:-50}"
PROMPT="${2:-echo ok}"
PHI_BIN="${PHI_BIN:-./target/release/phi}"
TEMP_DIR=$(mktemp -d)
PASSED=0
FAILED=0

echo "=== phi-agent stress test ==="
echo "Concurrency: $CONCURRENCY"
echo "Prompt: '$PROMPT'"
echo "Temp dir: $TEMP_DIR"
echo ""

# Run N concurrent phi processes
for i in $(seq 1 "$CONCURRENCY"); do
    (
        # Each process gets its own session ID
        export PHI_SESSION_ID="stress-$i-$(date +%s%N)"
        if timeout 30 "$PHI_BIN" --session-id "$PHI_SESSION_ID" "$PROMPT" > "$TEMP_DIR/out-$i.log" 2>"$TEMP_DIR/err-$i.log"; then
            echo "PASS $i"
        else
            echo "FAIL $i (exit code $?)"
        fi
    ) &
done

# Wait for all background processes
wait

# Count results
PASSED=$(grep -c '^PASS' "$TEMP_DIR"/out-*.log 2>/dev/null || echo 0)
FAILED=$((CONCURRENCY - PASSED))

echo ""
echo "=== Results ==="
echo "Passed: $PASSED / $CONCURRENCY"
echo "Failed: $FAILED"

if [ "$FAILED" -gt 0 ]; then
    echo ""
    echo "Failure logs:"
    grep -l 'FAIL' "$TEMP_DIR"/out-*.log 2>/dev/null | while read -r f; do
        echo "  $f"
    done
fi

# Cleanup
rm -rf "$TEMP_DIR"

if [ "$FAILED" -gt 0 ]; then
    exit 1
fi
echo "All $CONCURRENCY processes completed successfully."
