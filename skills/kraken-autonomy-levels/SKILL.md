---
name: kraken-autonomy-levels
version: 1.0.0
description: "Progress from manual trading to full agent autonomy with controlled risk at each level."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-autonomy-levels

Use this skill to help a trader move from hands-on CLI usage to fully autonomous agent trading in safe, incremental steps.

## Level 1: Read-Only Agent

The agent reads market data and account state. No API key needed for public data; a query-only key for account data.

**Kraken API key permissions:** Query Funds, Query Open Orders & Trades

```bash
export KRAKEN_API_KEY="query-only-key"
export KRAKEN_API_SECRET="query-only-secret"

kraken ticker BTCUSD -o json 2>/dev/null
kraken balance -o json 2>/dev/null
kraken open-orders -o json 2>/dev/null
```

The agent can inform, alert, and recommend. It cannot place orders, cancel orders, or move funds.

## Level 2: Paper Trading Agent

The agent tests strategies against live prices with zero risk. No API key needed.

```bash
kraken workspace create sandbox --capital 10000 --mode paper -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json 2>/dev/null
kraken workspace status -o json 2>/dev/null
kraken workspace reset -o json --yes 2>/dev/null
```

Run paper strategies repeatedly. Compare results across parameter changes. Only move to Level 3 after consistent paper performance.

## Level 3: Supervised Trading Agent

The agent places real orders, but a human confirms each one. The API key has trading permissions, but the agent validates before executing and waits for human approval.

**Kraken API key permissions:** Query Funds, Create & Modify Orders, Cancel/Close Orders

```bash
export KRAKEN_API_KEY="trade-key"
export KRAKEN_API_SECRET="trade-secret"

# Step 1: Agent validates the order (no submission)
kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json 2>/dev/null

# Step 2: Agent presents the validated order to the human
# Step 3: Human approves or rejects

# Step 4: Agent executes only after approval
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json 2>/dev/null
```

Safety controls at this level:
- Always `--validate` before executing
- Always enable the dead man's switch for the session: `kraken order cancel-after 300 -o json`
- Check `open-orders` after every trade to verify state

## Level 4: Autonomous Trading Agent

The agent trades independently. No human in the loop. The trader trusts the agent and its strategy.

**Kraken API key permissions:** Create & Modify Orders, Cancel/Close Orders, Query Funds

```bash
export KRAKEN_API_KEY="full-trade-key"
export KRAKEN_API_SECRET="full-trade-secret"
```

The agent passes `--yes` to skip confirmation prompts and operates on its own schedule.

**Required safeguards at this level:**

1. **Restricted API key.** Never use a key with withdrawal permissions for an autonomous agent. Create a key that can trade but cannot withdraw funds.

2. **Dead man's switch.** Start every session with `cancel-after` so all orders auto-cancel if the agent crashes or disconnects:
   ```bash
   kraken order cancel-after 600 -o json 2>/dev/null
   ```
   Refresh periodically. If the agent stops refreshing, all orders cancel.

3. **Position limits in agent code.** The CLI does not enforce position size or frequency limits. Build these in your agent logic:
   - Maximum order size per trade
   - Maximum open positions
   - Maximum trades per hour
   - Pair allowlist (only trade specific pairs)

4. **Error handling.** Autonomous agents must handle every error category:
   - `rate_limit`: back off, reduce frequency
   - `network`: retry with backoff, do not double-submit orders
   - `auth`: stop trading, alert the trader
   - `validation`: log and skip, do not retry unchanged

5. **Monitoring.** Run a separate read-only agent (Level 1) that monitors positions and alerts the trader if thresholds are breached.

## Level 5: Autonomous Agent with Fund Management

The agent can also move funds (deposits, withdrawals, transfers between wallets). This is the highest trust level.

**Kraken API key permissions:** All permissions including Withdraw Funds

This level is rare and high-risk. Only appropriate when:
- The agent manages a treasury or rebalances across exchanges
- The withdrawal addresses are pre-approved and whitelisted in Kraken's account settings
- Kraken's withdrawal address lock is enabled (prevents adding new addresses via API)

## Choosing the Right Level

| Level | Agent can | Human involvement | Risk |
|-------|-----------|-------------------|------|
| 1 | Read data | None | Zero |
| 2 | Paper trade | None | Zero |
| 3 | Trade with approval | Confirms every order | Low |
| 4 | Trade autonomously | Monitors, intervenes if needed | Medium |
| 5 | Trade and move funds | Monitors | High |

## Progression Rule

Move to the next level only after the agent demonstrates consistent behavior at the current level. Paper trading (Level 2) for at least a week before supervised trading (Level 3). Supervised trading for at least a week before autonomous (Level 4).

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
