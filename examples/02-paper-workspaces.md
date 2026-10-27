# Paper workspaces

A workspace is an isolated trading account: its own capital, fee rate, slippage
model, trade history, and recorded sessions. Create as many as you like and test
different ideas side by side without them contaminating each other.

## Create one

```bash
kraken workspace create btc-momentum --capital 10000 --mode paper
```

Capital, currency, and the economics are set at creation:

```bash
kraken workspace create realistic \
  --capital 25000 \
  --mode paper \
  --fee-rate 0.0026 \
  --slippage-rate 0.001
```

The fee rate defaults to 0.26% (Kraken's standard taker fee) and slippage to
zero. `--allow-pairs BTCUSD,ETHUSD` restricts what the workspace may trade.

## Activate a paper workspace and trade

Activate a workspace with the `--workspace` flag or the environment variable:

```bash
export KRAKEN_WORKSPACE=btc-momentum

kraken workspace show
kraken order buy BTCUSD 0.05 --type market               # simulated
kraken order sell BTCUSD 0.02 --type limit --price 70000 # simulated
kraken open-orders                                       # local paper orders
kraken trades-history                                    # simulated fills
```

At this point `btc-momentum` is in `paper` mode, so the workspace-aware `order`,
`open-orders`, and `trades-history` commands use its local simulator. They do
not submit private order requests to Kraken.

The examples deliberately use `kraken order` instead of `kraken paper` because
`order` is the command surface that follows the workspace mode:

| Active scope                    | `kraken order buy` behavior                                       |
| ------------------------------- | ----------------------------------------------------------------- |
| Named workspace in `paper` mode | Simulates the order against that workspace's allocation           |
| Named workspace in `live` mode  | Submits a real Spot order, charged to that workspace's allocation |
| No named workspace (`default`)  | Submits a real Spot order against the whole account               |

The explicit `kraken paper buy`, `kraken paper orders`, and
`kraken paper history` commands remain available for working directly with the
simulator, and they are refused inside a live workspace — a simulated fill in a
ledger that is supposed to hold only real executions would corrupt its P&L. Do
not treat `kraken order` as inherently safe: confirm `kraken workspace show`
reports `mode: paper` first.

Fills execute at live market prices with your workspace's fee and slippage
applied. The `--reason "breakout above resistance"` flag on buy/sell logs your
rationale alongside the trade, which pays off later when you ask the CLI to
explain a session's P&L.

## Check where you stand

```bash
kraken workspace status      # capital, current value, unrealized P&L
kraken workspace balance     # per-asset balances
kraken workspace list        # every workspace, plus 'default' (the real account)
kraken workspace report      # one row per session: P&L, fills, verdict
```

The name `default` is reserved for your real Kraken account. Named workspaces
may be created as paper or live, but starting with paper keeps the
developer-to-trader path reviewable.

## The promotion gate

```bash
kraken workspace promote btc-momentum
```

Promotion walks the workspace's recorded evidence and prints a checklist:
experiments scored by the [autoresearch lab](05-autoresearch-lab.md), how many
sessions passed, whether a live-market session passed, and whether trades
happened outside recorded windows. When the checklist passes and the user
confirms, the same workspace starts sending real Spot orders through the normal
configured Kraken account.

This is the safety boundary: after promotion, repeating the earlier
`kraken order buy` command places a real order. `KRAKEN_WORKSPACE` still names
`btc-momentum`, but its mode has changed from `paper` to `live`. Check
`kraken workspace show` before the first post-promotion order — the journal
records a `fill_source_changed` event at that boundary, so simulated history
stays distinguishable from real history.

A live workspace is an allocation of your account, not a subaccount. Promotion
verifies the account can cover the workspace's capital, and thereafter the CLI
refuses orders the allocation cannot fund. Kraken enforces none of this: funds
and permissions stay shared, so anything trading the same API key can spend
allocated money. Every live read shows the allocation against the real balance:

```bash
kraken workspace balance -o json | jq '.backing'
# { "status": "backed", "asset": "USD", "allocation": "10000",
#   "account": "50000.0000", "allocated_elsewhere": "0",
#   "unallocated": "40000.0000", "spendable": "9500" }
```

`spendable` is what the workspace's own ledger still holds free of resting
orders. If the status turns `over_allocated` — or `unverified`, when the real
balance cannot be read — the workspace stops opening positions until the account
covers the allocation again.

## Reset or start over

```bash
kraken workspace reset --yes    # new capital epoch; history survives
```

A reset can also change what creation set: pass `--capital`, `--currency`,
`--fee-rate`, or `--slippage-rate` for the new epoch. Without them, the new
epoch starts again from the capital, currency, and rates on record: the creation
values, or the last reset's overrides.

Everything a workspace owns lives in one directory under your config dir
(`workspaces/<name>/`): the account journal, the workspace manifest, and a
`sessions/` folder. Delete the directory and the workspace is gone.

## Hand it to an agent

The [`kraken-paper-strategy`](../skills/kraken-paper-strategy/SKILL.md) skill
teaches an agent to test strategy logic inside a workspace, and
[`kraken-paper-to-live`](../skills/kraken-paper-to-live/SKILL.md) walks the
promotion gate, evidence checklist and all. With the skills installed
([how](06-agent-skills.md)):

> _"Create a paper workspace with $10k and realistic fees, trade a simple
> mean-reversion idea on BTC for 20 minutes, and report the workspace status."_

Next: give your workspaces something to work with —
[a local library of recorded market data](03-market-memory.md).
