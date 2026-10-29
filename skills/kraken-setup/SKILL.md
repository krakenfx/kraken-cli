---
name: kraken-setup
version: 1.1.0
description: "Install kraken-cli, create API credentials, and go from paper trading to live in under five minutes."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-setup

First-run setup for `kraken-cli`. Covers install, credentials, paper trading, and handing off to an MCP client.

Use this skill when a user says "set up kraken-cli", "install kraken", "get started", or similar. For wiring the running CLI into Claude, Cursor, Codex, or Gemini, hand off to `kraken-mcp-integration`.

## 1. Install the CLI

The one-liner installs `kraken`, `kraken-mcp` (the MCP server) and
`kraken-shell` (an interactive shell) into `~/.cargo/bin`, or `$CARGO_HOME/bin` if `CARGO_HOME` is set:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/krakenfx/kraken-cli/releases/latest/download/kraken-cli-installer.sh | sh
```

From source (Rust 1.97 or later), the same three binaries:

```bash
cargo install --locked --git https://github.com/krakenfx/kraken-cli kraken-cli
```

Prebuilt archives are on [GitHub Releases](https://github.com/krakenfx/kraken-cli/releases). Move
`kraken`, `kraken-mcp` and `kraken-shell` into a directory on `$PATH`.

The installer adds `~/.cargo/bin` (or `$CARGO_HOME/bin`) to PATH for new terminals only. If `kraken` is not found right after install, call it by its full path, `~/.cargo/bin/kraken` or `$CARGO_HOME/bin/kraken`, and
tell the user to restart the agent from a new terminal window.

Verify:

```bash
kraken --version && kraken-mcp --version
```

## 2. Start with paper trading

Paper trading uses live prices with a simulated balance. No API keys required. Start here.

```bash
kraken workspace create sandbox --capital 10000 --mode paper -o json
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json
kraken workspace status -o json
```

Futures paper trading has one account that no workspace scopes, and it refuses
changes while a workspace is selected, so clear it first:

```bash
unset KRAKEN_WORKSPACE
kraken futures paper init --balance 10000 -o json
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json
kraken futures paper positions -o json
```

Public market data also needs no credentials:

```bash
kraken ticker BTCUSD -o json
kraken orderbook BTCUSD --count 10 -o json
```

Most agent workflows can do their entire first session on paper and public data.

## 3. (Optional) Add API credentials for live trading

Only needed if you want to query your real account or place real orders.

### Recommended key scope

Generate the narrowest key you can live with. An AI agent should never be able to move funds off the exchange.

- **Never enable `Withdraw Funds`.** Without this permission, even a prompt-injection or credential-exfiltration attack cannot remove assets from your Kraken account.
- **Start read-only.** `Query Funds`, `Query Open Orders & Trades`, and `Query Closed Orders & Trades` are safe for a first session.
- **Add trading scopes only when you need them.** `Create & Modify Orders` and `Cancel/Close Orders` unlock live trading; no other scope is required for spot.
- **Enable IP allowlisting if the agent runs from a static IP.** Kraken's API settings page lets you lock a key to one or more source IPs.
- **Rotate on host change.** If you switch machines or re-install the plugin, revoke the old key and generate a new one.

Manage keys at <https://www.kraken.com/u/security/api>.

### Provide the key and secret to your agent

The CLI resolves credentials from three tiers, in order: command-line flags (`--api-key` / `--api-secret`), environment variables (`KRAKEN_API_KEY` / `KRAKEN_API_SECRET`), then the active stored key in a secure local config file managed by the CLI (stored with user-only permissions under your OS's standard config directory). The stored key is either the key from `kraken auth set` or `kraken setup`, or the sign-in key from `kraken auth login`; `kraken auth switch` selects which one. When an active sign-in key expires, private commands fail with an `auth` error and a hint to sign in again or switch. The CLI does not fall back to the other key. Pick the path that matches your client.

#### Claude Code plugin

The `kraken-cli` plugin starts `kraken-mcp -s market,paper,feedback` and asks for no API key: none of those tools needs one. `kraken-mcp` still reads the active stored key from the CLI's local config, so a key saved earlier with `kraken auth set` or `kraken auth login` on the same machine is available to it. To keep the plugin credential-less, run `kraken auth reset` before starting Claude.

#### Cursor, Codex, Gemini CLI

Like the Claude plugin, these hosts start `kraken-mcp` with no key of their own. Either export credentials in the shell that launches the agent:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

…or rely on the key you stored with `kraken auth set` or `kraken auth login` — the plugin reads the same config file as the standalone CLI, so one stored key serves both, and `kraken auth switch` changes it for both.

For futures, use a Kraken Futures key and set `KRAKEN_FUTURES_API_KEY` and `KRAKEN_FUTURES_API_SECRET` alongside the spot pair, or store both pairs once with `kraken auth set --api-key <spot-key> --api-secret-file <spot-secret-file> --futures-api-key <futures-key> --futures-api-secret-stdin` and let the config file supply them. `auth set` always saves the spot pair too, so `--api-key` is required.

#### Plain CLI use (not through a plugin)

`kraken auth set` writes credentials to a secure local config file managed by the CLI (stored with user-only permissions in your OS's standard config directory). `kraken auth login` instead signs you in through the browser and stores the Spot API key it creates there: no withdrawal permissions, and an expiry date (90 days unless `--valid-for` says otherwise). It needs a terminal, so a person runs it, never an agent. `kraken auth show` displays the stored keys masked as `****wxyz` and the secret as `[REDACTED]`, and names the key in use, so you can confirm which key signs without exposing it. `kraken auth reset` clears the stored credentials; a sign-in key stays valid on Kraken until it expires.

### Verify

```bash
kraken balance -o json
```

Never commit these values. `.env` and `.env.*` are already gitignored in the repo.

## 4. Connect an MCP client

The CLI ships with a built-in MCP server. Point any MCP-compatible client (Claude Code, Claude Desktop, Cursor, Codex, Gemini CLI, VS Code, Windsurf) at it.

See `skills/kraken-mcp-integration/SKILL.md` for the per-client config blocks and the full service reference.

The default MCP config in every `kraken-cli` marketplace listing (Claude, Cursor, Codex, Gemini) is `-s market,paper,feedback`:

| Service | Auth | Why it's in the default |
|---------|------|-------------------------|
| `market` | None | Public price data, orderbooks, OHLC. |
| `paper`  | None | Simulated trading with live prices. |
| `feedback` | None | Lets the agent report friction or bugs. It uploads the text it writes, or a local file it names, to Kraken with no approval step; drop it from `-s` for an agent that reads untrusted input. |

A fresh install can never place a real order. Live trading is an explicit
opt-in: widen the service list in your MCP client's config to `-s market,trade,paper`
(adds live spot orders) or `-s all` (everything, including funding and earn).
See the service table and the "Enabling live trading" section in
`skills/kraken-mcp-integration/SKILL.md` for the complete list and recommended
progression.

## 5. Safety defaults

- Commands flagged `dangerous` in `agents/tool-catalog.json` require explicit user confirmation. MCP surfaces this via the `destructive_hint` annotation.
- Always try `--validate` before submitting a spot order. Futures orders have no dry run, so test them on `kraken futures paper` first.
- Run paper-first, live-second. `skills/kraken-paper-to-live/SKILL.md` covers the promotion checklist.

## Troubleshooting

**`kraken: command not found`**: the install directory isn't on `$PATH` yet. Open a new terminal, or call `~/.cargo/bin/kraken` (or `$CARGO_HOME/bin/kraken`) by its full path.

**`auth` error on every command**: you exported a key with insufficient permissions. Regenerate with the scopes above.

**Futures commands fail with `auth` but spot works**: futures needs its own key. Set `KRAKEN_FUTURES_API_KEY` and `KRAKEN_FUTURES_API_SECRET`.

**MCP client shows no Kraken tools**: confirm `kraken-mcp` is on the PATH the client sees (MCP clients often start with a minimal environment), or set `command` to the absolute path that `command -v kraken-mcp` prints in a new terminal. Most clients start the server without a shell, so `~` and `$CARGO_HOME` stay unexpanded. Releases before this one installed `kraken` only: run the one-liner again. Then see `kraken-mcp-integration` troubleshooting.

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
