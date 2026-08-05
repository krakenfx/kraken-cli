#!/usr/bin/env bash
# Portfolio Rebalance (Paper Trading)
# Targets 60/30/10 allocation across BTC/ETH/SOL using paper trading.
# Requirements: kraken (on PATH), jq, bc
set -euo pipefail

INITIAL_BALANCE=50000
TARGET_BTC=60
TARGET_ETH=30
TARGET_SOL=10

echo "=== Portfolio Rebalance (Paper) ==="
echo "Balance: \$${INITIAL_BALANCE}"
echo "Target: BTC ${TARGET_BTC}% / ETH ${TARGET_ETH}% / SOL ${TARGET_SOL}%"
echo ""

# Initialize
echo "Step 1: Initialize paper account"
kraken paper init --balance "$INITIAL_BALANCE" -o json 2>/dev/null | jq '.'
echo ""

# Get prices
echo "Step 2: Fetch prices"
BTC_PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price // "0"')
ETH_PRICE=$(kraken ticker ETHUSD -o json 2>/dev/null | jq -r '.[].last_price // "0"')
SOL_PRICE=$(kraken ticker SOLUSD -o json 2>/dev/null | jq -r '.[].last_price // "0"')
echo "  BTC: \$$BTC_PRICE  ETH: \$$ETH_PRICE  SOL: \$$SOL_PRICE"
echo ""

if [ "$BTC_PRICE" = "0" ] || [ "$ETH_PRICE" = "0" ] || [ "$SOL_PRICE" = "0" ]; then
  echo "Error: Could not fetch prices." && exit 1
fi

# Calculate volumes
echo "Step 3: Calculate target volumes"
BTC_ALLOC=$(echo "scale=2; $INITIAL_BALANCE * $TARGET_BTC / 100" | bc -l)
ETH_ALLOC=$(echo "scale=2; $INITIAL_BALANCE * $TARGET_ETH / 100" | bc -l)
SOL_ALLOC=$(echo "scale=2; $INITIAL_BALANCE * $TARGET_SOL / 100" | bc -l)
BTC_VOL=$(echo "scale=6; $BTC_ALLOC / $BTC_PRICE" | bc -l)
ETH_VOL=$(echo "scale=6; $ETH_ALLOC / $ETH_PRICE" | bc -l)
SOL_VOL=$(echo "scale=6; $SOL_ALLOC / $SOL_PRICE" | bc -l)
echo "  BTC: \$$BTC_ALLOC -> ${BTC_VOL} BTC"
echo "  ETH: \$$ETH_ALLOC -> ${ETH_VOL} ETH"
echo "  SOL: \$$SOL_ALLOC -> ${SOL_VOL} SOL"
echo ""

# Execute paper trades
echo "Step 4: Execute paper trades"
echo "  Buying BTC..."
kraken paper buy BTCUSD "$BTC_VOL" -o json 2>/dev/null | jq -r '.order_id // .id // "done"'
echo "  Buying ETH..."
kraken paper buy ETHUSD "$ETH_VOL" -o json 2>/dev/null | jq -r '.order_id // .id // "done"'
echo "  Buying SOL..."
kraken paper buy SOLUSD "$SOL_VOL" -o json 2>/dev/null | jq -r '.order_id // .id // "done"'
echo ""

# Verify
echo "Step 5: Verify portfolio"
echo "Balances:"
kraken paper balance -o json 2>/dev/null | jq '.'
echo ""
echo "Status:"
kraken paper status -o json 2>/dev/null | jq '.'
echo ""

# Actual allocation
echo "Step 6: Allocation check"
BTC_VAL=$(echo "scale=2; $BTC_VOL * $BTC_PRICE" | bc -l)
ETH_VAL=$(echo "scale=2; $ETH_VOL * $ETH_PRICE" | bc -l)
SOL_VAL=$(echo "scale=2; $SOL_VOL * $SOL_PRICE" | bc -l)
INVESTED=$(echo "scale=2; $BTC_VAL + $ETH_VAL + $SOL_VAL" | bc -l)
echo "  BTC: $(echo "scale=1; $BTC_VAL / $INVESTED * 100" | bc -l)% (target ${TARGET_BTC}%)"
echo "  ETH: $(echo "scale=1; $ETH_VAL / $INVESTED * 100" | bc -l)% (target ${TARGET_ETH}%)"
echo "  SOL: $(echo "scale=1; $SOL_VAL / $INVESTED * 100" | bc -l)% (target ${TARGET_SOL}%)"
echo ""

echo "=== Rebalance complete ==="
