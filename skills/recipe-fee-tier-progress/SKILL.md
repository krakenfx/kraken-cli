---
name: recipe-fee-tier-progress
version: 1.0.0
description: "Track 30-day trading volume progress toward the next fee tier."
metadata:
  openclaw:
    category: "recipe"
    domain: "portfolio"
  requires:
    bins: ["kraken"]
    skills: ["kraken-fee-optimization"]
---

# Fee Tier Progress

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-fee-optimization`

Check current 30-day volume and calculate how far you are from the next fee tier.

## Steps

1. Get current volume and fee tier: `kraken volume --pair BTCUSD -o json 2>/dev/null`
2. Extract `volume` (30-day USD) and current `fee` rates
3. Look up the next tier threshold from the fee schedule
4. Calculate remaining volume needed: next_threshold - current_volume
5. Estimate days to reach at current daily average
6. Check futures volume if applicable: `kraken futures fee-schedule-volumes -o json 2>/dev/null`
7. Present summary: current tier, current volume, next tier threshold, volume remaining, estimated days

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
