# Paper workspaces

A workspace is an isolated trading account: its own capital, fee rate, slippage model, trade history, and recorded sessions. Create as many as you like and test different ideas side by side without them contaminating each other.

## Create one

```bash
kraken workspace create btc-momentum --capital 10000 --mode paper
```

Capital and currency are fixed at creation, as are the economics:

```bash
kraken workspace create realistic \
  --capital 25000 \
  --mode paper \
  --fee-rate 0.0026 \
  --slippage-rate 0.001
```

The fee rate defaults to 0.26% (Kraken's standard taker fee) and slippage to zero. `--allow-pairs BTCUSD,ETHUSD` restricts what the workspace may trade.

## Activate and trade

Activate a workspace with the `--workspace` flag or the environment variable:

```bash
export KRAKEN_WORKSPACE=btc-momentum

kraken paper buy BTCUSD 0.05
kraken paper sell BTCUSD 0.02 --type limit --price 70000
kraken paper orders
kraken paper history
```

Fills execute at live market prices with your workspace's fee and slippage applied. The `--reason "breakout above resistance"` flag on buy/sell logs your rationale alongside the trade, which pays off later when you ask the CLI to explain a session's P&L.

## Check where you stand

```bash
kraken workspace status      # capital, current value, unrealized P&L
kraken workspace balance     # per-asset balances
kraken workspace list        # every workspace, plus 'default' (the real account)
kraken workspace report      # one row per session: P&L, fills, verdict
```

The name `default` is reserved for your real Kraken account. Every other workspace is paper until it earns promotion.

## The promotion gate

```bash
kraken workspace promote btc-momentum
```

Promotion toward live trading is not a flag flip. The command walks the workspace's recorded evidence and prints a checklist: experiments scored by the [autoresearch lab](05-autoresearch-lab.md), how many sessions passed, whether a live-market session passed, and whether trades happened outside recorded windows. A workspace with no evidence gets a clear list of what's missing rather than a promotion.

## Reset or start over

```bash
kraken workspace reset --yes    # wipe history, return to the created capital
```

Everything a workspace owns lives in one directory under your config dir (`workspaces/<name>/`): the account journal, the workspace manifest, and a `sessions/` folder. Delete the directory and the workspace is gone.

## Hand it to an agent

The [`kraken-paper-strategy`](../skills/kraken-paper-strategy/SKILL.md) skill teaches an agent to test strategy logic inside a workspace, and [`kraken-paper-to-live`](../skills/kraken-paper-to-live/SKILL.md) walks the promotion gate, evidence checklist and all. With the skills installed ([how](06-agent-skills.md)):

> *"Create a paper workspace with $10k and realistic fees, trade a simple mean-reversion idea on BTC for 20 minutes, and report the workspace status."*

Next: give your workspaces something to work with — [a local library of recorded market data](03-market-memory.md).
