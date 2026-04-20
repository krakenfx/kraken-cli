---
name: kraken-mcp-integration
version: 1.0.0
description: "Connect MCP clients to kraken-cli for native tool calling without subprocess wrappers."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-mcp-integration

Use this skill to connect any MCP-compatible client to `kraken-cli` for structured tool calling over stdio.

MCP tool calls execute through the same command path as CLI commands, so error handling and rate-limit behavior is identical between MCP and CLI.

## Supported Clients

Claude Desktop, ChatGPT, Codex, Gemini CLI, Cursor, VS Code, Windsurf, and any client that supports the MCP `mcpServers` configuration block.

## Setup

### 1. Configure your MCP client

Add this to your MCP client configuration (Claude Desktop: `claude_desktop_config.json`, Cursor: `.cursor/mcp.json`, etc.):

```json
{
  "mcpServers": {
    "kraken": {
      "command": "kraken",
      "args": ["mcp", "-s", "market,paper"]
    }
  }
}
```

`-s market,paper` is the default service list: public market data plus paper
trading. No real funds can be touched until you opt in explicitly by
expanding the service list. See [Enabling live trading](#enabling-live-trading)
below.

Credential resolution follows the standard CLI precedence: command-line
flags, then `KRAKEN_API_KEY` / `KRAKEN_API_SECRET` in the MCP server's `env`
block, then a secure local config file managed by the CLI (written by
`kraken auth set` and `kraken setup`, stored with user-only permissions in
your OS's standard config directory). The MCP server and the standalone CLI
share the same config file, so a single `kraken auth set` serves both.

### 2. Set credentials (only needed for live trading)

If you stay on `market,paper`, skip this step — paper trading needs no keys.

For live trading, the MCP server reads credentials from environment variables:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

### 3. Restart your MCP client

The client discovers tools on startup.

## Service Filtering

Control which command groups the MCP server exposes:

```bash
kraken mcp -s market                    # public data only (safe, no auth)
kraken mcp -s market,paper              # market data + paper trading (marketplace default)
kraken mcp -s market,trade,paper        # add live trading
kraken mcp -s market,account,trade      # add account queries and live trading
kraken mcp -s all                       # everything (many tools, includes funding/earn/subaccount)
```

Keep the service list to what you need. MCP clients typically handle 50-100 tools well.

| Service | Auth | Tools | Risk |
|---------|------|-------|------|
| market | No | ~10 | None |
| account | Yes | ~18 | Read-only |
| trade | Yes | ~9 | Orders (dangerous) |
| funding | Yes | ~10 | Withdrawals (dangerous) |
| earn | Yes | ~6 | Staking (dangerous) |
| subaccount | Yes | ~2 | Transfers (dangerous) |
| futures | Mixed | ~39 | Orders (dangerous) |
| paper | No | ~10 | None (simulation) |
| auth | No | ~3 | Read-only (auth set/auth reset excluded) |

## Enabling live trading

Every `kraken-cli` marketplace listing ships with `-s market,paper` by default,
so a fresh install can never place a real order. To opt in to live trading,
edit the MCP server args in your client's config and widen the service list:

```json
{
  "mcpServers": {
    "kraken": {
      "command": "kraken",
      "args": ["mcp", "-s", "market,trade,paper"],
      "env": {
        "KRAKEN_API_KEY": "your-key",
        "KRAKEN_API_SECRET": "your-secret"
      }
    }
  }
}
```

Recommended upgrade path:

1. Start with `market,paper` (the default).
2. Add `trade` once you are comfortable and have an API key scoped to orders
   only. See the "Recommended key scope" section of `skills/kraken-setup/SKILL.md`
   before generating a key.
3. Add `account` if you want the agent to read balances and positions.
4. Only enable `funding`, `earn`, `subaccount`, or `all` if you specifically
   need those workflows. Each of these unlocks tools that can move funds off
   the exchange or alter account structure.

Switching to `-s all` expands the set of tools the agent can reach by
prompt-injection or mistake. Prefer a narrow list and widen it deliberately.

## Safety

Dangerous tools include `[DANGEROUS: requires human confirmation]` in their description and carry the MCP `destructive_hint` annotation. In guarded mode (default), calls must include `acknowledged=true`. In autonomous mode (`--allow-dangerous`), the per-call confirmation is disabled. MCP clients that respect annotations may still prompt at the client layer.

For fully autonomous operation, see `skills/kraken-autonomy-levels/SKILL.md`.

## Gemini CLI Extension

If using Gemini CLI, install the extension directly:

```bash
gemini extensions install https://github.com/krakenfx/kraken-cli
```

The `gemini-extension.json` includes `mcpServers` config, so the MCP server starts automatically.

## Troubleshooting

**"No tools found"**: Check that `kraken` is on your PATH and the service list is valid. Run `kraken mcp -s market` manually to verify it starts.

**Auth errors on tool calls**: Set `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` in the environment where your MCP client runs.

**Too many tools**: Reduce the service list. `kraken mcp -s market,trade` is a good starting point.

**Streaming commands not available**: WebSocket streaming commands are excluded from MCP v1. Use `kraken ws ...` directly from the terminal for streaming.
