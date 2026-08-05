---
name: kraken-tax-export
version: 1.0.0
description: "Export trade history, ledgers, and cost basis data for tax reporting."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-portfolio-intel"]
---

# kraken-tax-export

Use this skill for:
- exporting trades and ledgers for tax preparation
- generating CSV exports for import into tax software
- computing realized gains from trade history
- auditing deposits and withdrawals for completeness

## Trade Export

Request a full trade export for a tax year:

```bash
kraken export-report --report trades --description "2024 tax year" --format CSV --starttm 1704067200 --endtm 1735689600 -o json 2>/dev/null
```

Check status:

```bash
kraken export-status --report trades -o json 2>/dev/null
```

Download when ready:

```bash
kraken export-retrieve <REPORT_ID> -o json 2>/dev/null
```

Clean up after download:

```bash
kraken export-delete <REPORT_ID> -o json 2>/dev/null
```

## Ledger Export

Ledgers capture all account activity (trades, deposits, withdrawals, staking rewards, fees):

```bash
kraken export-report --report ledgers --description "2024 full ledger" --format CSV --starttm 1704067200 --endtm 1735689600 -o json 2>/dev/null
```

## JSON Trade History (Alternative)

For smaller date ranges, query directly:

```bash
kraken trades-history --ledgers --consolidate-taker -o json 2>/dev/null
```

Filter by time:

```bash
kraken closed-orders --start 1704067200 --end 1735689600 -o json 2>/dev/null
```

## Futures History

Futures trades are exported separately:

```bash
kraken futures history-executions --since 2024-01-01T00:00:00Z --before 2025-01-01T00:00:00Z --sort asc -o json 2>/dev/null
```

Futures account log (CSV):

```bash
kraken futures history-account-log-csv --since 2024-01-01T00:00:00Z --before 2025-01-01T00:00:00Z -o json 2>/dev/null
```

## Deposit and Withdrawal Records

Complete picture for tax reconciliation:

```bash
kraken deposit status --start 1704067200 --end 1735689600 -o json 2>/dev/null
kraken withdrawal status --start 1704067200 --end 1735689600 -o json 2>/dev/null
```

## Earn/Staking Rewards

Staking rewards appear as ledger entries of type `staking`:

```bash
kraken ledgers --type staking --start 1704067200 --end 1735689600 -o json 2>/dev/null
```

## Cost Basis Workflow

1. Export all trades for the tax year (CSV).
2. Import into tax software (Koinly, CoinTracker, etc.).
3. Verify deposit/withdrawal records match exchange data.
4. Cross-reference staking rewards from ledger entries.
5. Generate tax report.

## Hard Rules

- Requesting and reading exports is read-only, but `export-retrieve` (writes to disk) and `export-delete` (irreversible) are flagged dangerous. Never delete a report without explicit human approval.
- Verify export completeness by checking trade counts against `trades-history` totals.
- Keep exports secure; they contain full trading history.
- Futures and spot exports are separate; ensure both are included.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
