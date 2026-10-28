#!/usr/bin/env bash
# Paper Trading Workflow
# Demonstrates: init, buy, check status, limit sell, check orders, reset
# Requirements: kraken (on PATH), jq, bc
set -euo pipefail

PAIR="BTCUSD"
INITIAL_BALANCE=10000
BUY_VOLUME="0.01"
# A workspace of its own keeps the script off the global paper account and rerunnable.
export KRAKEN_WORKSPACE="paper-workflow-example"

echo "=== Paper Trading Workflow ==="

# Step 1: Initialize (a rerun resets the workspace instead)
echo "Step 1: Initialize paper account with \$${INITIAL_BALANCE}"
if kraken workspace show -o json >/dev/null 2>&1; then
  # --yes: the reset only touches this script's own paper workspace.
  INIT=$(kraken paper reset --balance "$INITIAL_BALANCE" --yes -o json 2>/dev/null)
else
  INIT=$(kraken paper init --balance "$INITIAL_BALANCE" -o json 2>/dev/null)
fi
echo "$INIT" | jq '.'

# Step 2: Get current price
echo "Step 2: Get current $PAIR price"
TICKER=$(kraken ticker "$PAIR" -o json 2>/dev/null)
PRICE=$(echo "$TICKER" | jq -r '.[].last_price // "unknown"')
echo "  Current price: \$$PRICE"

# Step 3: Market buy
echo "Step 3: Paper buy $BUY_VOLUME $PAIR at market"
BUY_RESULT=$(kraken paper buy "$PAIR" "$BUY_VOLUME" -o json 2>/dev/null)
echo "$BUY_RESULT" | jq '.'

# Step 4: Check portfolio
echo "Step 4: Check portfolio status"
kraken paper status -o json 2>/dev/null | jq '.'

# Step 5: Check balances
echo "Step 5: Check balances"
kraken paper balance -o json 2>/dev/null | jq '.'

# Step 6: Place limit sell at 5% above market
echo "Step 6: Place limit sell at 5% above market"
if command -v bc >/dev/null 2>&1 && [ "$PRICE" != "unknown" ]; then
  SELL_PRICE=$(echo "$PRICE * 1.05" | bc -l | xargs printf "%.1f")
  echo "  Sell price: \$$SELL_PRICE"
  kraken paper sell "$PAIR" "0.005" --type limit --price "$SELL_PRICE" -o json 2>/dev/null | jq '.'
else
  echo "  Skipping (bc not available or price unknown)"
fi

# Step 7: Check open orders
echo "Step 7: Check open orders"
kraken paper orders -o json 2>/dev/null | jq '.'

# Step 8: Trade history
echo "Step 8: Check trade history"
kraken paper history -o json 2>/dev/null | jq '.'

# Step 9: Reset
echo "Step 9: Reset paper account"
kraken paper reset --yes -o json 2>/dev/null | jq '.'

echo "=== Workflow complete ==="
