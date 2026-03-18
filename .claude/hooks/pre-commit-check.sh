#!/bin/bash
set -euo pipefail

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only intercept git commit commands
if ! echo "$COMMAND" | grep -q "git commit"; then
  exit 0
fi

echo "Running pre-commit checks..." >&2

# Check formatting
if ! cargo fmt --check 2>/dev/null; then
  echo "BLOCKED: cargo fmt check failed. Run: cargo fmt" >&2
  exit 2
fi

# Run all tests (lib + bin)
if ! cargo test --quiet 2>/dev/null; then
  echo "BLOCKED: cargo test failed. Fix tests before committing." >&2
  exit 2
fi

echo "Pre-commit checks passed (fmt + 198 tests)." >&2
exit 0
