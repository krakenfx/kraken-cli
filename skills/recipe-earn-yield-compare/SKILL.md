---
name: recipe-earn-yield-compare
version: 1.0.0
description: "Compare earn strategy yields across assets and lock types to find the best rate."
metadata:
  openclaw:
    category: "recipe"
    domain: "earn"
  requires:
    bins: ["kraken"]
    skills: ["kraken-earn-staking"]
---

# Compare Earn Yields

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-earn-staking`

Compare available staking/earn strategies across assets to find the best risk-adjusted yield.

## Steps

1. List strategies for multiple assets:
   - `kraken earn strategies --asset ETH -o json 2>/dev/null`
   - `kraken earn strategies --asset DOT -o json 2>/dev/null`
   - `kraken earn strategies --asset SOL -o json 2>/dev/null`
   - `kraken earn strategies --asset ATOM -o json 2>/dev/null`
2. Extract `apr_estimate`, `lock_type`, `min_amount`, `can_allocate` from each
3. Separate by lock type: flexible vs bonded
4. Rank by APR within each lock type
5. Present comparison table: asset, APR, lock type, minimum amount, availability
6. Check current allocations: `kraken earn allocations --hide-zero-allocations --converted-asset USD -o json 2>/dev/null`
7. Identify reallocation opportunities (move from lower to higher yield)
8. If user approves, deallocate from lower yield and allocate to higher yield

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
