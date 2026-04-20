---
name: kraken-setup
version: 1.0.0
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

Pick one. All three land a `kraken` binary on your PATH.

```bash
# Homebrew (macOS, Linux)
brew tap krakenfx/kraken-cli && brew install kraken-cli

# Cargo (any platform with Rust >= 1.77)
cargo install kraken-cli

# Prebuilt binary
# Download from https://github.com/krakenfx/kraken-cli/releases and move `kraken` into a directory on $PATH
```

Verify:

```bash
kraken --version
```

## 2. Start with paper trading

Paper trading uses live prices with a simulated balance. No API keys required. Start here.

```bash
kraken paper init --balance 10000 -o json
kraken paper buy BTCUSD 0.01 -o json
kraken paper status -o json
```

Futures paper trading works the same way:

```bash
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

The CLI resolves credentials from three tiers, in order: command-line flags (`--api-key` / `--api-secret`), environment variables (`KRAKEN_API_KEY` / `KRAKEN_API_SECRET`), then a secure local config file managed by the CLI (written by `kraken auth set` or `kraken setup`, stored with user-only permissions under your OS's standard config directory). Pick the path that matches your client.

#### Claude Code (install-time prompt)

When you enable the `kraken-cli` plugin from the Plugin Directory, Claude prompts for `api_key` and `api_secret` and stores them in your macOS Keychain. Paste the key and secret, or press Enter on both prompts to stay on paper + market-only. Claude wires the values into the MCP server's environment automatically; no shell exports required.

If you leave the `userConfig` fields blank, the plugin starts with no environment credentials and falls through to whatever the CLI's local config file contains. That means a user who previously ran `kraken auth set` on the same machine will have those CLI credentials available to the plugin. If you want the plugin to be strictly credential-less until you opt in, leave the prompt blank AND clear the config file with `kraken auth reset` before starting Claude.

#### Cursor, Codex, Gemini CLI

These hosts do not have Claude's install-time prompt. Either export credentials in the shell that launches the agent:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

…or rely on the credentials you stored with `kraken auth set` — the plugin reads the same config file as the standalone CLI, so a single `kraken auth set` serves both.

For futures, use a Kraken Futures key and set `KRAKEN_FUTURES_API_KEY` and `KRAKEN_FUTURES_API_SECRET` alongside the spot pair, or run `kraken auth set --futures-api-key ... --futures-api-secret ...` once and let the config file supply them.

#### Plain CLI use (not through a plugin)

`kraken auth set` writes credentials to a secure local config file managed by the CLI (stored with user-only permissions in your OS's standard config directory). `kraken auth show` displays the stored key masked as `****wxyz` and the secret as `[REDACTED]`, so you can confirm which key is configured without exposing it. `kraken auth reset` clears the stored credentials.

### Verify

```bash
kraken balance -o json
```

Never commit these values. `.env` and `.env.*` are already gitignored in the repo.

## 4. Connect an MCP client

The CLI ships with a built-in MCP server. Point any MCP-compatible client (Claude Code, Claude Desktop, Cursor, Codex, Gemini CLI, VS Code, Windsurf) at it.

See `skills/kraken-mcp-integration/SKILL.md` for the per-client config blocks and the full service reference.

The default MCP config in every `kraken-cli` marketplace listing (Claude, Cursor, Codex, Gemini) is `-s market,paper`:

| Service | Auth | Why it's in the default |
|---------|------|-------------------------|
| `market` | None | Public price data, orderbooks, OHLC. |
| `paper`  | None | Simulated trading with live prices. |

A fresh install can never place a real order. Live trading is an explicit
opt-in: widen the service list in your MCP client's config to `-s market,trade,paper`
(adds live spot orders) or `-s all` (everything, including funding and earn).
See the service table and the "Enabling live trading" section in
`skills/kraken-mcp-integration/SKILL.md` for the complete list and recommended
progression.

## 5. Safety defaults

- Commands flagged `dangerous` in `agents/tool-catalog.json` require explicit user confirmation. MCP surfaces this via the `destructive_hint` annotation.
- Always try `--validate` before submitting an order.
- Run paper-first, live-second. `skills/kraken-paper-to-live/SKILL.md` covers the promotion checklist.

## Troubleshooting

**`kraken: command not found`**: the install directory isn't on `$PATH`. For Homebrew: run `brew doctor`. For Cargo: ensure `~/.cargo/bin` is on `$PATH`.

**`auth` error on every command**: you exported a key with insufficient permissions. Regenerate with the scopes above.

**Futures commands fail with `auth` but spot works**: futures needs its own key. Set `KRAKEN_FUTURES_API_KEY` and `KRAKEN_FUTURES_API_SECRET`.

**MCP client shows no Kraken tools**: confirm `kraken` is on the PATH the client sees (MCP clients often launch from a minimal shell). Then see `kraken-mcp-integration` troubleshooting.
