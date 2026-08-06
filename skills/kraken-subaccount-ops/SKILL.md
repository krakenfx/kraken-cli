---
name: kraken-subaccount-ops
version: 1.0.0
description: "Create and manage subaccounts with inter-account transfers."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-subaccount-ops

Use this skill for:
- creating subaccounts for strategy isolation
- transferring funds between main and subaccounts
- managing futures subaccount trading status
- isolating risk across multiple strategies

## Why Subaccounts

Subaccounts isolate balances and positions. Use them to:
- Run different strategies with separate capital pools.
- Limit risk exposure per strategy.
- Track P&L per strategy independently.
- Give different agents access to different subaccounts with restricted API keys.

## Create a Subaccount

```bash
kraken subaccount create "dca-bot" "dca-bot@example.com" -o json 2>/dev/null
```

## Transfer Between Accounts

Move funds from main account to subaccount (requires human approval):

```bash
kraken subaccount transfer USD 5000 --from <MAIN_IIBAN> --to <SUB_IIBAN> -o json 2>/dev/null
```

Move funds back:

```bash
kraken subaccount transfer USD 5000 --from <SUB_IIBAN> --to <MAIN_IIBAN> -o json 2>/dev/null
```

## Futures Subaccounts

List futures subaccounts:

```bash
kraken futures subaccounts -o json 2>/dev/null
```

Check subaccount trading status:

```bash
kraken futures subaccount-status <UID> -o json 2>/dev/null
```

Enable or disable trading for a subaccount:

```bash
kraken futures set-subaccount-status <UID> true -o json 2>/dev/null
kraken futures set-subaccount-status <UID> false -o json 2>/dev/null
```

Transfer between futures wallets:

```bash
kraken futures wallet-transfer <FROM_WALLET> <TO_WALLET> USD 1000 -o json 2>/dev/null
```

## Strategy Isolation Pattern

1. Create a subaccount per strategy (e.g., "grid-btc", "dca-eth").
2. Allocate capital to each subaccount.
3. Configure separate API keys per subaccount with minimum required permissions.
4. Each agent operates only within its assigned subaccount.
5. The main account retains reserve capital not allocated to any strategy.

## Monitoring

Check all balances from the main account to see aggregate state. Individual subaccount balances are visible through their respective API keys.

For centralized monitoring, use the main account's credentials with query permissions across subaccounts.

## Hard Rules

- Subaccount transfers and `subaccount create` are flagged as dangerous. Never execute without explicit human approval — a created subaccount cannot be removed through the API.
- Never share API keys across subaccounts; create separate keys per subaccount.
- Verify IIBAN values before initiating transfers; incorrect IIBANs will fail.
- Disable futures trading on subaccounts that should not trade futures.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
