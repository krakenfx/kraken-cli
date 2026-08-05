---
name: recipe-withdrawal-to-cold-storage
version: 1.0.0
description: "Safely withdraw funds to a pre-approved cold storage address."
metadata:
  openclaw:
    category: "recipe"
    domain: "funding"
  requires:
    bins: ["kraken"]
    skills: ["kraken-funding-ops"]
---

# Withdraw to Cold Storage

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-funding-ops`

Withdraw crypto to a pre-approved hardware wallet address with fee checks and status tracking.

> **CAUTION:** Withdrawals are irreversible once confirmed on-chain. Triple-check the address.

## Steps

1. Check balance for the asset: `kraken balance -o json 2>/dev/null`
2. List verified withdrawal addresses: `kraken withdrawal addresses --asset BTC --verified true -o json 2>/dev/null`
3. Confirm the target address matches the cold storage address on file
4. Check withdrawal fee: `kraken withdrawal info BTC "cold-storage-btc" 0.5 -o json 2>/dev/null`
5. Present fee and net amount to the user
6. Execute withdrawal (requires explicit human approval): `kraken withdraw BTC "cold-storage-btc" 0.5 -o json 2>/dev/null`
7. Track withdrawal status: `kraken withdrawal status --asset BTC -o json 2>/dev/null`
8. Verify balance reduction: `kraken balance -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
