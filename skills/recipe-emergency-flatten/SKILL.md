---
name: recipe-emergency-flatten
version: 1.0.0
description: "Cancel all orders and close all positions in an emergency."
metadata:
  openclaw:
    category: "recipe"
    domain: "risk"
  requires:
    bins: ["kraken"]
    skills: ["kraken-risk-operations", "kraken-liquidation-guard"]
---

# Emergency Flatten

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-risk-operations`, `kraken-liquidation-guard`

Immediately cancel all orders and close all positions across spot and futures.

> **CAUTION:** This closes everything at market prices. Slippage may be significant. The `--yes` flag skips confirmation prompts and is only appropriate at autonomy level 4+ (see `kraken-autonomy-levels`). At lower levels, omit `--yes` and confirm each cancel with the user.

## Steps

1. Cancel all spot orders (dangerous): `kraken order cancel-all --yes -o json 2>/dev/null`
2. Cancel all futures orders (dangerous): `kraken futures cancel-all --yes -o json 2>/dev/null`
3. Check spot positions: `kraken positions -o json 2>/dev/null`
4. Close each spot margin position with a market order (requires human approval)
5. Check futures positions and extract sizes: `kraken futures positions -o json 2>/dev/null` (parse each position's `size` and `side` fields)
6. Close each futures position with reduce-only market orders (use `sell` for longs, `buy` for shorts): e.g., `kraken futures order sell PF_XBTUSD 1 --reduce-only -o json 2>/dev/null`
7. Verify no open orders remain: `kraken open-orders -o json 2>/dev/null`
8. Verify no futures orders remain: `kraken futures open-orders -o json 2>/dev/null`
9. Verify all positions closed: `kraken futures positions -o json 2>/dev/null`
10. Check final balances: `kraken balance -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
