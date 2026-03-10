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
      "args": ["mcp", "-s", "market,trade,paper"]
    }
  }
}
```

### 2. Set credentials

The MCP server reads credentials from environment variables:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

Public market data and paper trading require no credentials.

### 3. Restart your MCP client

The client discovers tools on startup.

## Service Filtering

Control which command groups the MCP server exposes:

```bash
kraken mcp -s market                    # public data only (safe, no auth)
kraken mcp -s market,paper              # market data + paper trading
kraken mcp -s market,trade,paper        # add live trading
kraken mcp -s market,account,trade      # add account queries
kraken mcp -s all                       # everything (many tools)
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
