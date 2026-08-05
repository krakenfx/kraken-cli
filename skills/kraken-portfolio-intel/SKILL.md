---
name: kraken-portfolio-intel
version: 1.0.0
description: "Portfolio analysis, P&L tracking, trade history, and export reports."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-portfolio-intel

Use this skill for:
- reading balances and computing portfolio value
- tracking trade history and P&L
- analyzing fee impact and volume tiers
- exporting reports for external analysis

## Balance Overview

Simple balance:

```bash
kraken balance -o json 2>/dev/null
```

Extended balance (includes credit and held amounts):

```bash
kraken extended-balance -o json 2>/dev/null
```

Trade balance (margin, equity, free margin):

```bash
kraken trade-balance --asset USD -o json 2>/dev/null
```

## Portfolio Valuation

Combine balances with current prices to compute total portfolio value:

```bash
# Get all balances
kraken balance -o json 2>/dev/null

# Get prices for held assets
kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null

# Agent calculates:
# value_per_asset = balance * price
# total_value = sum of all values
# weight_per_asset = value / total_value
```

## Trade History

Recent trades:

```bash
kraken trades-history -o json 2>/dev/null
```

With ledger entries for cost basis:

```bash
kraken trades-history --ledgers -o json 2>/dev/null
```

Consolidated taker trades (merge partial fills):

```bash
kraken trades-history --consolidate-taker -o json 2>/dev/null
```

Time-filtered:

```bash
kraken closed-orders --start 1704067200 --end 1706745600 -o json 2>/dev/null
```

Query specific trades by TXID:

```bash
kraken query-trades <TXID1> <TXID2> -o json 2>/dev/null
```

## Ledger Analysis

View all ledger entries (trades, deposits, withdrawals, staking):

```bash
kraken ledgers -o json 2>/dev/null
```

Filter by type:

```bash
kraken ledgers --type trade -o json 2>/dev/null
kraken ledgers --type deposit -o json 2>/dev/null
kraken ledgers --type withdrawal -o json 2>/dev/null
kraken ledgers --type staking -o json 2>/dev/null
```

Filter by asset:

```bash
kraken ledgers --asset BTC -o json 2>/dev/null
```

Time range:

```bash
kraken ledgers --asset BTC --type trade --start 1704067200 --end 1706745600 -o json 2>/dev/null
```

## Fee Analysis

Check current volume and fee tier:

```bash
kraken volume --pair BTCUSD -o json 2>/dev/null
```

The response includes 30-day volume and the corresponding maker/taker fee rates.

## Export Reports

Request a trade or ledger export for offline analysis:

```bash
kraken export-report --report trades --description "Q1 2024 trades" --format CSV --starttm 1704067200 --endtm 1711929600 -o json 2>/dev/null
```

Check export status:

```bash
kraken export-status --report trades -o json 2>/dev/null
```

Download when ready:

```bash
kraken export-retrieve <REPORT_ID> -o json 2>/dev/null
```

Clean up old reports (`export-delete` is flagged dangerous — the report is gone and must be regenerated, so confirm with the user first):

```bash
kraken export-delete <REPORT_ID> -o json 2>/dev/null
```

## Futures Portfolio

Futures account summary:

```bash
kraken futures accounts -o json 2>/dev/null
```

Futures fill history:

```bash
kraken futures fills --since 2024-01-01T00:00:00Z -o json 2>/dev/null
```

Futures execution history:

```bash
kraken futures history-executions --sort desc -o json 2>/dev/null
```

Futures account log (CSV export):

```bash
kraken futures history-account-log-csv --since 2024-01-01T00:00:00Z -o json 2>/dev/null
```

## Open Positions

Spot margin positions:

```bash
kraken positions --show-pnl -o json 2>/dev/null
```

Futures positions:

```bash
kraken futures positions -o json 2>/dev/null
```

## Context Efficiency

- Use `--offset` and `--limit` for paginated queries.
- Use `--without-count` on large history queries to skip count computation.
- Filter by asset and time range to reduce response size.
- Use exports for bulk data instead of paginating through REST.

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
