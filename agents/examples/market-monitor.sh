#!/usr/bin/env bash
# Market Monitor
# Monitors prices, volumes, spreads, and order book depth for multiple pairs.
# Requirements: kraken (on PATH), jq, bc
set -euo pipefail

PAIRS="BTCUSD ETHUSD SOLUSD"
SPREAD_THRESHOLD="0.5"

echo "=== Market Monitor ==="
echo "Pairs: $PAIRS"
echo "Spread alert threshold: ${SPREAD_THRESHOLD}%"
echo ""

# System status
echo "--- System Status ---"
kraken status -o json 2>/dev/null | jq -r '"Status: \(.status // "unknown")"'
echo ""

# Ticker data
echo "--- Ticker Data ---"
for PAIR in $PAIRS; do
  TICKER=$(kraken ticker "$PAIR" -o json 2>/dev/null)
  ASK=$(echo "$TICKER" | jq -r '.[].ask_price // "N/A"')
  BID=$(echo "$TICKER" | jq -r '.[].bid_price // "N/A"')
  LAST=$(echo "$TICKER" | jq -r '.[].last_price // "N/A"')
  VOL=$(echo "$TICKER" | jq -r '.[].volume_24h // "N/A"')
  echo "$PAIR: Ask=$ASK Bid=$BID Last=$LAST Vol24h=$VOL"
  if command -v bc >/dev/null 2>&1 && [ "$ASK" != "N/A" ] && [ "$BID" != "N/A" ]; then
    SPREAD=$(echo "scale=4; ($ASK - $BID) / $BID * 100" | bc -l 2>/dev/null || echo "N/A")
    echo "  Spread: ${SPREAD}%"
    ALERT=$(echo "$SPREAD > $SPREAD_THRESHOLD" | bc -l 2>/dev/null || echo "0")
    [ "$ALERT" = "1" ] && echo "  ** ALERT: Spread exceeds threshold **"
  fi
done
echo ""

# Order book depth
echo "--- BTCUSD Order Book (top 5) ---"
BOOK=$(kraken orderbook BTCUSD --count 5 -o json 2>/dev/null)
echo "Asks:" && echo "$BOOK" | jq -r '.asks[:5][] | "  \(.price) @ \(.volume)"' 2>/dev/null || echo "  (error)"
echo "Bids:" && echo "$BOOK" | jq -r '.bids[:5][] | "  \(.price) @ \(.volume)"' 2>/dev/null || echo "  (error)"
echo ""

# Recent trades
echo "--- BTCUSD Recent Trades (last 5) ---"
TRADES=$(kraken trades BTCUSD --count 5 -o json 2>/dev/null)
echo "$TRADES" | jq -r '.trades[:5][] | "\(.time) \(if .side == "buy" then "BUY " else "SELL" end) \(.volume) @ \(.price)"' 2>/dev/null || echo "  (error)"
echo ""

# OHLC
echo "--- BTCUSD Hourly OHLC (last 3) ---"
OHLC=$(kraken ohlc BTCUSD --interval 60 -o json 2>/dev/null)
echo "$OHLC" | jq -r '.candles[-3:][] | "  O:\(.open) H:\(.high) L:\(.low) C:\(.close) V:\(.volume)"' 2>/dev/null || echo "  (error)"
echo ""

echo "=== Monitor complete ==="
