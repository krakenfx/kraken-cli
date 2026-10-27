# Recipes and extensions

Walkthrough 4 left you with a stopped, recorded session; walkthrough 5 graded
one. This one packages that work so it survives your shell history: a **recipe**
re-runs yesterday's setup with one command, and an **extension** consumes the
CLI's JSON output without forking anything.

Nothing here needs a Kraken account. Runs happen in paper workspaces against the
public feed.

## Package a session as a recipe

Any stopped session in a paper workspace can become a recipe. The sessions from
walkthroughs 4 and 5 carry no strategy label, so `--driver` names the skill that
drives the re-run:

```bash
export KRAKEN_WORKSPACE=btc-momentum
kraken recipe create btc-levels --from-session latest --driver recipe-playground-price-alert -o json
```

That writes one portable directory under the global config dir:

```text
recipes/btc-levels/
├── recipe.json   # driver skill, frozen params, symbols, channels, sinks, provenance
└── SKILL.md      # a draft an agent can load — marked DRAFT, never authored logic
```

The artifact is an editable input: tuning `params` in `recipe.json` between runs
is the intended workflow, so nothing is sealed (the lab's experiments keep that
role). A session with a strategy label (from `--strategy`, or any playground
session) needs no `--driver`. An existing recipe name refuses unless you pass
`--overwrite`.

Find it again later:

```bash
kraken recipe list -o json
kraken recipe show btc-levels -o json
```

A fresh install already has one entry: the built-in `playground` recipe, the
one-command demo's twin. An on-disk recipe of the same name shadows it.

## Run it again

`recipe run` is the inverse of `recipe create` — it writes exactly the flags
create read and starts an ordinary session:

```bash
kraken recipe run btc-levels --for 15m
```

The run provisions (or reuses — never re-funds) a paper workspace named after
the recipe, seeded from the artifact's capital, then hands off on stderr: the
driver skill to load, the params, and the watch/trade/review commands. Because
the run is a normal session, everything you already know works on it:

```bash
KRAKEN_WORKSPACE=btc-levels kraken session show -o json    # includes strategy + params
KRAKEN_WORKSPACE=btc-levels kraken explain pnl -o json
KRAKEN_WORKSPACE=btc-levels kraken lab score -o json
KRAKEN_WORKSPACE=btc-levels kraken lab curve -o json       # the full marked equity series
```

Replays work too — `kraken recipe run btc-levels --from tape:<name> --speed 10`
— and `--experiment <name>` stamps the session so `lab compare` discovers it.
Running against the real account is a deliberate, confirmed opt-in
(`--real-account`) that prints its evidence caveats up front; it refuses
`--from`, because recorded prices cannot settle honestly on a ledger of real
executions.

## Scaffold an extension

There is no plugin ABI: the CLI's versioned JSON output is the extension API.
`plugin init` scaffolds a working client so you start from running code:

```bash
kraken plugin init volume-profile --lang python --kind indicator
```

```text
plugins/volume-profile/
├── extension.json   # identity for `kraken plugin list` (deliberately not plugin.json)
├── main.py          # dependency-free; shells out to `kraken ohlc ... -o json`
├── SKILL.md         # loadable by an agent runtime
└── README.md        # the contract, stated
```

The entrypoint runs unmodified against live public market data:

```bash
python3 "$(kraken plugin init sma --lang python -o json | jq -r .path)/main.py" BTC/USD
{"pair": "BTC/USD", "candles": 42, "sma20": 30240.7}
```

`--kind alert|exporter|driver` tailors the example (threshold check, CSV export,
a signal an agent turns into action — the extension itself never places orders),
and `--lang bash` generates a JSON passthrough for shell pipelines.
`kraken plugin list` inventories what you scaffolded.

## Hand it to an agent

The generated `SKILL.md` files are standard skill packages. Symlink a recipe or
extension into your agent's skills directory (see
[walkthrough 6](06-agent-skills.md)) and the prompt becomes:

> _"Run my `btc-levels` recipe for 30 minutes, then score the session and tell
> me if the levels held."_
