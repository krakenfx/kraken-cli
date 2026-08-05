# Agent skills

Everything in walkthroughs 1 through 5 can be driven by an AI agent instead of typed by hand. The repo ships 58 skill packages under [skills/](../skills/) — structured instructions that teach an agent the CLI's contracts, safety rules, and workflows, so a plain-language prompt becomes a correct sequence of commands.

Each skill is a standard `SKILL.md` package and works with any agent runtime that reads them: Claude Code, OpenClaw, and others. The full catalog is in the [Skills Index](../skills/INDEX.md).

## Install

Symlink or copy the skills into your agent's skills directory. For OpenClaw:

```bash
cd /path/to/kraken-cli
for skill in skills/kraken-* skills/recipe-*; do
  ln -s "$(pwd)/$skill" ~/.openclaw/skills/
done
```

For Claude Code, copy the ones you want into `~/.claude/skills/` (personal) or your project's `.claude/skills/`:

```bash
cp -r skills/kraken-playground skills/kraken-lab-experiment skills/recipe-session-post-mortem ~/.claude/skills/
```

Agents can also reach the same features over MCP (`kraken mcp`) instead of the shell; the [`kraken-mcp-integration`](../skills/kraken-mcp-integration/SKILL.md) skill covers that path.

## The journey, as prompts

With the skills installed, the manual walkthroughs collapse into conversation. All of it stays on paper accounts:

> *"Start a playground session and paper trade BTC for 15 minutes. Show me your P&L."*
> — [`kraken-playground`](../skills/kraken-playground/SKILL.md) creates the sandbox and records the window ([walkthrough 1](01-playground.md))

> *"Record two ten-minute windows of BTC and ETH market data, morning and afternoon."*
> — the tapes land in your local library ([walkthrough 3](03-market-memory.md))

> *"I think buying dips and taking profit on BTC beats sitting in cash. Register that as a lab experiment, screen it on both recorded windows, confirm live for 20 minutes, and give me the verdict."*
> — [`kraken-lab-experiment`](../skills/kraken-lab-experiment/SKILL.md) freezes the hypothesis and criteria, works the sealed session plan, and reports the mechanical pass/fail ([walkthrough 5](05-autoresearch-lab.md))

> *"Do a post-mortem on the live session. Where did every dollar go?"*
> — [`recipe-session-post-mortem`](../skills/recipe-session-post-mortem/SKILL.md) replays the window and narrates the P&L decomposition ([walkthrough 4](04-sessions-replay-explain.md))

> *"The experiment passed. What would it take to go live?"*
> — [`kraken-paper-to-live`](../skills/kraken-paper-to-live/SKILL.md) walks the promotion checklist against the recorded evidence ([walkthrough 2](02-paper-workspaces.md#the-promotion-gate))

The agent has no shortcut around the gates. Lab criteria are hash-sealed before the first session, tapes are content-hashed, and promotion reads recorded evidence from disk. An agent that trades badly gets the same mechanical refusal you would.

## Running a hypothesis as a ReAct loop

The lab is deliberately shaped for an agent that works in a **ReAct loop** — reason, act, observe, repeat. The lab experiment prompt above isn't a single command the agent memorizes; it's a state machine the agent *queries the world for* on every turn. One command is the whole loop:

```bash
kraken lab next <experiment> -o json
```

`lab next` re-derives progress from the sessions directory on disk each time it's called. It never trusts what the agent thinks happened — it reads what actually happened and returns the next state: `start`, `running`, or `plan_complete`. That single fact is what makes the loop robust: **the observation is the filesystem, not the agent's memory.**

Here is one experiment playing out, turn by turn. The agent's private reasoning is *Thought*, the command it runs is *Action*, and what the CLI hands back — never what the agent assumes — is *Observation*.

**Setup (once).** The hypothesis is frozen with its pass criteria and session plan before anything runs, so the goalposts are nailed down:

```bash
kraken lab new dip-buy \
  --hypothesis "buying BTC dips and taking profit beats holding cash" \
  --min-return-pct 0.5 --max-drawdown-pct 2 --min-fills 4 \
  --session replay:tape:window-am \
  --session replay:tape:window-pm \
  --session live:20m
```

### Iteration 1 — screen on the first recorded window

- **Thought:** I don't know where this experiment stands. Ask the world, don't guess.
- **Action:** `kraken lab next dip-buy -o json`
- **Observation:** `{"state":"start","command":"kraken session start --label dip-buy-r1 --experiment dip-buy --from tape:window-am --speed 10 ..."}` — the plan's first entry is a replay screen.
- **Thought:** State is `start`. I run the exact command it gave me, drive my dip-buy logic while the tape replays, and log a reason on every fill so the scorecard can attribute it.
- **Action:** run that command; `kraken paper buy BTCUSD 0.05 --reason "dip below VWAP"`; `kraken session stop`; then `kraken lab score --session latest --experiment dip-buy -o json`
- **Observation:** `{"verdict":{"pass":false,"checks":[{"criterion":"min_return_pct","threshold":0.5,"observed":-0.31,"pass":false}]}}` — the waterfall shows the return went to fees. A round-trip dip-buy booked only the cost of trading.

### Iteration 2 — the loop asks again

- **Thought:** One window is done. I do not assume the next step — I ask.
- **Action:** `kraken lab next dip-buy -o json`
- **Observation:** `{"state":"start","command":"... --from tape:window-pm ..."}` — the second replay entry. The completed run consumed the first plan slot; a failed verdict does not stop the plan.
- The agent screens `window-pm` the same way, stops, and scores.

### Iteration 3 — live confirmation, then the wall

- **Action:** `kraken lab next dip-buy` returns `{"state":"start","command":"... --symbols BTC/USD --channels ticker,trade ..."}` — the `live:20m` entry. The agent trades the live window and scores it.
- **Action:** `kraken lab next dip-buy` now returns `{"state":"plan_complete"}`.
- **Thought:** Plan's done. The verdict is not mine to declare — I read it off `compare`.
- **Action:** `kraken lab compare dip-buy -o json`
- **Observation:** `{"pass_count":0,"total":3,"runs":[...]}` — zero of three cleared the sealed bar.

**Conclusion.** The agent reports what the CLI produced: the hypothesis failed, the return floor was the binding check, and the money went to fees on every window. It cannot round that up. The skill's own rule is blunt about it — *never report a verdict you did not obtain from `lab score` or `lab compare` in this session; no estimating, no "it would probably pass."* The mechanical gate is what keeps the agent honest.

### Why this shape matters

- **The loop is grounded, not remembered.** Because `lab next` reads state from disk, the agent can lose its context, crash, or be handed off to a fresh session between any two turns and still resume exactly where the files say it left off. Whatever step is half-done on disk *is* the next step.
- **It runs unattended.** Drive the same skill under [`/loop`](../skills/kraken-lab-experiment/SKILL.md) and each firing advances the machine by one step — `start` a session, or stop-and-score a finished one, or conclude on `plan_complete` — then exits. A firing that finds nothing to advance does nothing. No babysitting a long live window.
- **The agent can't move the goalposts.** Criteria and the session plan are hash-sealed at `lab new`; each replay tape is content-hashed, so a re-recorded window is refused outright. The agent's judgement decides how to *trade*; the frozen spec decides whether it *passed*.

If the numbers hold — at least two passing sessions including a live one — the same loop hands off to [`kraken-paper-to-live`](../skills/kraken-paper-to-live/SKILL.md), which walks the promotion checklist against exactly this recorded evidence. The path from a sentence-long hunch to real capital runs entirely through observe-think-act, and every gate on it is mechanical.

## Guardrails worth knowing

- [`kraken-autonomy-levels`](../skills/kraken-autonomy-levels/SKILL.md) defines graduated permission tiers, from read-only market data up to live execution, so you decide how much rope the agent gets.
- [`kraken-shared`](../skills/kraken-shared/SKILL.md) is the base contract every skill builds on: JSON output, error categories, and the rule that dangerous commands need explicit confirmation.
- The lab lifecycle is resumable from disk at every step, so long-running experiments survive agent restarts and context loss.

## Beyond the walkthroughs

The catalog goes well past what these examples cover: futures trading and liquidation guards, DCA and grid strategies, fee optimization, portfolio intel, tax exports, and 25 ready-made recipes from morning market briefs to drawdown circuit breakers. Browse the [Skills Index](../skills/INDEX.md).
