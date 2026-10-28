#!/usr/bin/env bash
# Portfolio Rebalance (Paper Trading)
# Targets 60/30/10 allocation across BTC/ETH/SOL using paper trading.
# Requirements: kraken (on PATH), jq, bc
set -euo pipefail

INITIAL_BALANCE=50000
TARGET_BTC=60
TARGET_ETH=30
TARGET_SOL=10
# Held back so a price move between quote and fill cannot overdraw the last buy.
CASH_BUFFER_PCT=1
# A workspace of its own keeps the script off the global paper account and rerunnable.
export KRAKEN_WORKSPACE="portfolio-rebalance-example"

echo "=== Portfolio Rebalance (Paper) ==="
echo "Balance: \$${INITIAL_BALANCE}"
echo "Target: BTC ${TARGET_BTC}% / ETH ${TARGET_ETH}% / SOL ${TARGET_SOL}%"
echo ""

# Initialize (a rerun resets the workspace instead)
echo "Step 1: Initialize paper account"
if kraken workspace show -o json >/dev/null 2>&1; then
  # --yes: the reset only touches this script's own paper workspace.
  kraken paper reset --balance "$INITIAL_BALANCE" --yes -o json 2>/dev/null | jq '.'
else
  kraken paper init --balance "$INITIAL_BALANCE" -o json 2>/dev/null | jq '.'
fi
FEE_RATE=$(kraken workspace show -o json 2>/dev/null | jq -r '.fee_rate')
echo ""

# Market buys fill at the ask, so size the legs on it.
echo "Step 2: Fetch ask prices"
BTC_PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].ask_price // "0"')
ETH_PRICE=$(kraken ticker ETHUSD -o json 2>/dev/null | jq -r '.[].ask_price // "0"')
SOL_PRICE=$(kraken ticker SOLUSD -o json 2>/dev/null | jq -r '.[].ask_price // "0"')
echo "  BTC: \$$BTC_PRICE  ETH: \$$ETH_PRICE  SOL: \$$SOL_PRICE"
echo ""

if [ "$BTC_PRICE" = "0" ] || [ "$ETH_PRICE" = "0" ] || [ "$SOL_PRICE" = "0" ]; then
  echo "Error: Could not fetch prices." && exit 1
fi

# Calculate volumes net of the fee the engine charges on top of each fill
echo "Step 3: Calculate target volumes (fee rate $FEE_RATE, ${CASH_BUFFER_PCT}% cash buffer)"
INVESTABLE=$(echo "scale=2; $INITIAL_BALANCE * (100 - $CASH_BUFFER_PCT) / 100" | bc -l)
BTC_ALLOC=$(echo "scale=2; $INVESTABLE * $TARGET_BTC / 100" | bc -l)
ETH_ALLOC=$(echo "scale=2; $INVESTABLE * $TARGET_ETH / 100" | bc -l)
SOL_ALLOC=$(echo "scale=2; $INVESTABLE * $TARGET_SOL / 100" | bc -l)
BTC_VOL=$(echo "scale=6; $BTC_ALLOC / ($BTC_PRICE * (1 + $FEE_RATE))" | bc -l)
ETH_VOL=$(echo "scale=6; $ETH_ALLOC / ($ETH_PRICE * (1 + $FEE_RATE))" | bc -l)
SOL_VOL=$(echo "scale=6; $SOL_ALLOC / ($SOL_PRICE * (1 + $FEE_RATE))" | bc -l)
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

# Actual allocation (multiply before dividing: bc truncates each step to scale)
echo "Step 6: Allocation check"
BTC_VAL=$(echo "scale=2; $BTC_VOL * $BTC_PRICE" | bc -l)
ETH_VAL=$(echo "scale=2; $ETH_VOL * $ETH_PRICE" | bc -l)
SOL_VAL=$(echo "scale=2; $SOL_VOL * $SOL_PRICE" | bc -l)
INVESTED=$(echo "scale=2; $BTC_VAL + $ETH_VAL + $SOL_VAL" | bc -l)
echo "  BTC: $(echo "scale=1; $BTC_VAL * 100 / $INVESTED" | bc -l)% (target ${TARGET_BTC}%)"
echo "  ETH: $(echo "scale=1; $ETH_VAL * 100 / $INVESTED" | bc -l)% (target ${TARGET_ETH}%)"
echo "  SOL: $(echo "scale=1; $SOL_VAL * 100 / $INVESTED" | bc -l)% (target ${TARGET_SOL}%)"
echo ""

echo "=== Rebalance complete ==="
