---
name: kraken-funding-ops
version: 1.0.0
description: "Manage deposits, withdrawals, and wallet transfers safely."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-funding-ops

Use this skill for:
- checking deposit methods and addresses
- tracking deposit and withdrawal status
- withdrawing funds to pre-approved addresses
- transferring between spot and futures wallets

## Deposit Workflow

1. Find available methods for an asset:
   ```bash
   kraken deposit methods BTC -o json 2>/dev/null
   ```
2. Get a deposit address:
   ```bash
   kraken deposit addresses BTC "Bitcoin" -o json 2>/dev/null
   ```
3. Generate a new address (if supported):
   ```bash
   kraken deposit addresses BTC "Bitcoin" --new -o json 2>/dev/null
   ```
4. Monitor incoming deposits:
   ```bash
   kraken deposit status --asset BTC -o json 2>/dev/null
   ```

Filter by time range:

```bash
kraken deposit status --asset BTC --start 1704067200 --end 1706745600 -o json 2>/dev/null
```

Paginate large result sets:

```bash
kraken deposit status --asset BTC --limit 25 --cursor <CURSOR> -o json 2>/dev/null
```

## Withdrawal Workflow

1. Check available methods:
   ```bash
   kraken withdrawal methods --asset BTC -o json 2>/dev/null
   ```
2. Check pre-approved addresses:
   ```bash
   kraken withdrawal addresses --asset BTC --verified true -o json 2>/dev/null
   ```
3. Get fee estimate:
   ```bash
   kraken withdrawal info BTC "my-btc-address" 0.5 -o json 2>/dev/null
   ```
4. Execute withdrawal (requires human approval):
   ```bash
   kraken withdraw BTC "my-btc-address" 0.5 -o json 2>/dev/null
   ```
5. Track status:
   ```bash
   kraken withdrawal status --asset BTC -o json 2>/dev/null
   ```

Cancel a pending withdrawal:

```bash
kraken withdrawal cancel BTC <REFID> -o json 2>/dev/null
```

## Wallet Transfer

Move funds between spot and futures wallets:

```bash
kraken wallet-transfer USD 1000 --from <SPOT_IIBAN> --to <FUTURES_IIBAN> -o json 2>/dev/null
```

Futures-specific transfer:

```bash
kraken futures transfer 1000 USD -o json 2>/dev/null
```

Check transfer history:

```bash
kraken futures transfers -o json 2>/dev/null
```

## Fee-Aware Withdrawal Pattern

Always check fees before withdrawing. Compare the fee to the withdrawal amount:

```bash
INFO=$(kraken withdrawal info BTC "my-btc-address" 0.5 -o json 2>/dev/null)
# Parse .fee and .limit from the response
# Present fee to user before proceeding
```

## Hard Rules

- Withdrawals are flagged as dangerous. Never execute without explicit human approval.
- Always verify the withdrawal address matches a pre-approved address in account settings.
- Use `--verified true` when listing addresses to confirm approval status.
- Check fees before every withdrawal; network fees fluctuate.
- Cancel pending withdrawals promptly if the user changes their mind: the window is short.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
