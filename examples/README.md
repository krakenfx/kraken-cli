# Examples

Guided walkthroughs for everything the CLI can do beyond single API calls. Each
one is a short, runnable session: the commands, what to expect, and why the step
matters.

None of them require a Kraken account. Market data comes from the public
WebSocket feed, and all trading happens in paper workspaces with simulated
fills.

Work through them in order and you go from "never traded" to a pre-registered
strategy experiment with a mechanical pass/fail verdict:

| #   | Walkthrough                                                        | What you build                                                                        | Time   |
| --- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------- | ------ |
| 1   | [The playground](01-playground.md)                                 | A live paper-trading sandbox from one command                                         | 2 min  |
| 2   | [Paper workspaces](02-paper-workspaces.md)                         | Isolated accounts with their own capital, fees, and history                           | 5 min  |
| 3   | [Market memory](03-market-memory.md)                               | A local library of recorded market data you can query and replay                      | 5 min  |
| 4   | [Sessions, replay, and P&L explain](04-sessions-replay-explain.md) | A recorded trading window you can rewind and decompose                                | 10 min |
| 5   | [The autoresearch lab](05-autoresearch-lab.md)                     | A sealed experiment that promotes a strategy only when the numbers hold               | 10 min |
| 6   | [Agent skills](06-agent-skills.md)                                 | The same journey driven by an AI agent, from one prompt                               | 5 min  |
| 7   | [Recipes and extensions](07-recipes.md)                            | A re-runnable package of a finished session, and a scaffolded JSON-contract extension | 5 min  |

Walkthroughs 1 to 5 and 7 end with a "Hand it to an agent" section: the CLI
ships 59 [agent skills](../skills/INDEX.md) that let an AI agent run the same
workflow from a plain-language prompt — including registering a trading
hypothesis in the lab and reporting the verdict. Walkthrough 6 shows how to set
that up.

Every command supports `-o json` for machine-readable output, so anything you
see here also works from a script. Ready-made agent scripts live in
[agents/examples/](../agents/examples/), and the same features are exposed to
MCP clients via `kraken-mcp`.

If you are new to the CLI itself (installation, market data, live trading),
start with the [README](../README.md).
