# The playground

One command gives you a live market feed, a paper account with $10,000, and a recorded session you can inspect afterwards. No account, no API keys.

```bash
kraken playground
```

What happens:

- A paper workspace named `playground` is created (or reused) with 10,000 USD of simulated capital.
- A recorded session starts inside it, streaming live BTC/USD ticker and trade data to a local DuckDB file.
- The stream prints to stdout as JSONL; guidance for what to do next prints to stderr.

The session stops itself after an hour. Pass `--for 10m` for a shorter window, or `--symbols BTC/USD,ETH/USD` to watch more pairs.

## Trade against it

In a second terminal, point your session at the playground workspace and trade:

```bash
export KRAKEN_WORKSPACE=playground

kraken order buy BTCUSD 0.01          # simulated fill at the live price
kraken workspace status               # capital, P&L, open orders
kraken session show                       # the recording window and its valuation
```

Inside a paper workspace, `kraken order buy/sell` executes simulated fills against real market prices, with fees applied. Nothing touches the live exchange.

## Stop and inspect

```bash
kraken session stop                       # close the window (or just wait for --for to elapse)
kraken session list                       # every session this workspace has recorded
```

Each run leaves a complete record behind: the market tape, your fills, and the account journal. That record is what [replay and P&L explain](04-sessions-replay-explain.md) work on.

## Clean up

```bash
kraken workspace reset --yes          # back to the starting capital
```

Or leave it. The playground is an ordinary workspace, and the [next walkthrough](02-paper-workspaces.md) shows what workspaces can do on their own.

## Hand it to an agent

Everything above is packaged as the [`kraken-playground`](../skills/kraken-playground/SKILL.md) skill, with ready-made strategy drivers like [`recipe-playground-dca`](../skills/recipe-playground-dca/SKILL.md) and [`recipe-playground-price-alert`](../skills/recipe-playground-price-alert/SKILL.md). With the skills installed ([how](06-agent-skills.md)), this is one prompt:

> *"Start a playground run for 15 minutes and DCA into BTC while it records. Show me the P&L when the window closes."*
