# The autoresearch lab

The lab turns "I have a hunch" into a pre-registered experiment: hypothesis, pass criteria, and session plan are frozen and hash-sealed before the first session, and the verdict is mechanical. You cannot move the goalposts, swap the data, or rubber-stamp a strategy that didn't earn it.

This walkthrough assumes a paper workspace ([walkthrough 2](02-paper-workspaces.md)) and at least two recorded tapes ([walkthrough 3](03-market-memory.md)), say `window-am` and `window-pm`.

```bash
export KRAKEN_WORKSPACE=btc-momentum
```

## Pre-register the experiment

```bash
kraken lab new dip-buy \
  --hypothesis "buying dips and taking profit on BTC beats sitting in cash" \
  --min-return-pct 0.5 \
  --max-drawdown-pct 2 \
  --min-fills 4 \
  --session replay:tape:window-am \
  --session replay:tape:window-pm \
  --session live:20m
```

This freezes three things at once:

- **The criteria** — a real return floor, a drawdown ceiling, a minimum trade count.
- **The session plan** — screen on two recorded windows first, then confirm on a live one. Order matters and is sealed.
- **The data** — each replay entry carries a content hash of its tape. If a single byte of a recorded window changes later, the lab refuses to run against it and names both hashes.

`kraken lab show dip-buy` displays the frozen experiment and verifies the seal.

## Work the plan

```bash
kraken lab next dip-buy
```

`lab next` tells you exactly which run comes next and prints the `kraken session start` command to execute, with `--experiment dip-buy` stamped in so results attach to the experiment. Run it, trade the idea (yourself, from a script, or with an agent driving the CLI), stop the window, and ask for the score:

```bash
kraken lab score --session latest --experiment dip-buy
```

The scorecard combines the P&L decomposition with mechanical metrics — return, max drawdown, fill count — and delivers a pass/fail verdict against the sealed criteria. Repeat `lab next` until the plan is complete.

## Compare and decide

```bash
kraken lab compare dip-buy
```

Every session of the experiment is re-scored against the sealed criteria and laid out side by side: recorded windows next to the live confirmation, each with its own verdict. The hypothesis holds or it doesn't, and the output shows why.

## From verdict to promotion

The lab's verdicts are the evidence [`kraken workspace promote`](02-paper-workspaces.md#the-promotion-gate) reads. Promotion requires experiments with multiple passing sessions including a live pass, so the path from idea to real capital runs through this loop by construction:

```text
hypothesis → sealed plan → replay screens → live confirmation → compare → promote
```

If the numbers don't hold, the gate says no, mechanically, with nothing to argue about. That refusal is the product.

## Hand it to an agent

This loop was built to be driven by an agent. The [`kraken-lab-experiment`](../skills/kraken-lab-experiment/SKILL.md) skill runs the whole lifecycle — freeze, run, score, verdict — and every step reads and writes plain files on disk, so the agent can be killed and restarted at any point and resume from whatever it finds half-done. With the skills installed ([how](06-agent-skills.md)), your trading hypothesis is one prompt:

> *"I think buying BTC dips and taking profit beats holding cash. Register that as a lab experiment with a 0.5% return floor, screen it on my two recorded windows, confirm on a 20-minute live session, and give me the verdict."*

The agent trades the idea inside each window; the sealed criteria judge it. The agent cannot talk its way past the gate any more than you can.

That is the whole journey. [Walkthrough 6](06-agent-skills.md) covers installing the skills and what else ships in the catalog.
