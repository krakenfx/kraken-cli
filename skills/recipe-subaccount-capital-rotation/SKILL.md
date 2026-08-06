---
name: recipe-subaccount-capital-rotation
version: 1.0.0
description: "Rotate capital between subaccounts based on strategy performance."
metadata:
  openclaw:
    category: "recipe"
    domain: "portfolio"
  requires:
    bins: ["kraken"]
    skills: ["kraken-subaccount-ops", "kraken-portfolio-intel"]
---

# Subaccount Capital Rotation

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-subaccount-ops`, `kraken-portfolio-intel`

Evaluate strategy performance per subaccount and reallocate capital to winning strategies.

> **CAUTION:** Transfers move real funds. Always confirm IIBANs and amounts.

## Steps

1. Check main account balance: `kraken balance -o json 2>/dev/null`
2. Check each subaccount balance (via their API keys or main account view)
3. Review trade history per subaccount to calculate P&L
4. Rank strategies by risk-adjusted return
5. Calculate new allocation (increase capital for outperformers, reduce for underperformers)
6. Obtain IIBANs from account settings or the Kraken web UI (IIBANs are account-specific identifiers like `ABCD 1234 EFGH 5678`)
7. Transfer from underperforming subaccount to main (requires human approval): `kraken subaccount transfer USD 2000 --from $SUB_IIBAN --to $MAIN_IIBAN -o json 2>/dev/null`
8. Transfer from main to outperforming subaccount: `kraken subaccount transfer USD 2000 --from $MAIN_IIBAN --to $SUB_IIBAN -o json 2>/dev/null`
9. Verify balances after transfers: `kraken balance -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
