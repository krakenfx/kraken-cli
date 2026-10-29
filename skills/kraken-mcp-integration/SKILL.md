---
name: kraken-mcp-integration
version: 1.1.0
description: "Connect MCP clients to kraken-cli for native tool calling without subprocess wrappers."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken-mcp"]
---

# kraken-mcp-integration

Use this skill to connect any MCP-compatible client to `kraken-cli` for structured tool calling over stdio.

MCP tool calls execute through the same command path as CLI commands, so error handling and rate-limit behavior is identical between MCP and CLI.

## Supported Clients

Claude Desktop, Codex, Gemini CLI, Cursor, VS Code, Windsurf, and other clients that start local stdio MCP servers. Most take an `mcpServers` JSON entry; VS Code and Codex use their own formats (below).

## Setup

### 1. Configure your MCP client

Add this to your MCP client configuration (Claude Desktop: `claude_desktop_config.json`, Cursor: `.cursor/mcp.json`, etc.):

```json
{
  "mcpServers": {
    "kraken": {
      "command": "kraken-mcp",
      "args": ["-s", "market,paper,feedback"]
    }
  }
}
```

VS Code takes the same entry under a top-level `"servers"` key in `.vscode/mcp.json`. Codex reads TOML in `~/.codex/config.toml`:

```toml
[mcp_servers.kraken]
command = "kraken-mcp"
args = ["-s", "market,paper,feedback"]
```

`-s market,paper,feedback` is the service list every marketplace listing ships:
public market data, paper trading, and the feedback tool, which uploads the text
it is given, or a local file it names, to Kraken. The file upload has no approval step, so a model can send any file it can name, including the CLI config file that holds your stored keys; drop `feedback` from `-s` for an agent that reads untrusted input. No real funds can be touched until you opt in explicitly by
expanding the service list. See [Enabling live trading](#enabling-live-trading)
below.

Started with no `-s`, `kraken-mcp` serves `market,account,paper,workspace,feedback`.
That adds account reads and the workspace, session and recipe tools, and with
them five gated tools: `export-delete`, `export-retrieve`, `workspace reset`,
`workspace promote`, and `workspace create` with mode `live`. `paper reset` is
gated in both sets.

Credential resolution follows the standard CLI precedence: command-line
flags, then `KRAKEN_API_KEY` / `KRAKEN_API_SECRET` in the MCP server's `env`
block, then the active stored key in a secure local config file managed by
the CLI (stored with user-only permissions in your OS's standard config
directory): the key from `kraken auth set` or `kraken setup`, or the sign-in
key from `kraken auth login`, as `kraken auth switch` selects. The MCP server
and the standalone CLI share the same config file, and the server reads it
again for each call, so a switch reaches a running server. When an active
sign-in key expires, calls that need a key fail with an `auth` error; the
server does not fall back to the other key.

### 2. Set credentials (only needed for live trading)

If you stay on `market,paper,feedback`, skip this step — paper trading needs no keys.

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
kraken-mcp -s market                    # public data only (safe, no auth)
kraken-mcp -s market,paper,feedback              # market data + paper trading (marketplace default)
kraken-mcp                              # binary default: market,account,paper,workspace,feedback
kraken-mcp -s market,trade,paper        # add live trading
kraken-mcp -s market,account,trade      # add account queries and live trading
kraken-mcp -s all                       # everything (152 tools, includes funding/earn/subaccount)
```

Keep the service list to what you need. MCP clients typically handle 50-100 tools well.

| Service | Auth | Tools | Risk |
|---------|------|-------|------|
| market | No | 11 | None |
| account | Yes | 18 | Reads, plus `export-delete` and `export-retrieve` (dangerous) |
| trade | Yes | 9 | Orders (dangerous) |
| paper | No | 17 | Simulation, lab and explain; `paper reset` (dangerous) |
| workspace | No | 20 | Workspaces, sessions, recipes, plugins; `workspace promote` and `reset` (dangerous), `workspace create` with mode `live` (gated) |
| futures | Mixed | 39 | Orders (dangerous) |
| futures-paper | No | 17 | None (simulation) |
| funding | Yes | 10 | Withdrawals (dangerous) |
| earn | Yes | 6 | Staking (dangerous) |
| auth | Mixed | 2 | Read-only (auth login/set/switch/reset excluded) |
| subaccount | Yes | 2 | Transfers (dangerous) |
| feedback | No | 1 | Uploads text or a local file to Kraken (no approval) |

The counts are the tools each service serves, 152 in all. Streaming, interactive and withheld commands (such as `auth switch`) run only from the CLI.

## Enabling live trading

Every `kraken-cli` marketplace listing ships with `-s market,paper,feedback` by default,
so a fresh install can never place a real order. To opt in to live trading,
edit the MCP server args in your client's config and widen the service list:

```json
{
  "mcpServers": {
    "kraken": {
      "command": "kraken-mcp",
      "args": ["-s", "market,trade,paper"],
      "env": {
        "KRAKEN_API_KEY": "your-key",
        "KRAKEN_API_SECRET": "your-secret"
      }
    }
  }
}
```

Recommended upgrade path:

1. Start with `market,paper,feedback` (the marketplace default).
2. Add `trade` once you are comfortable and have an API key scoped to orders
   only. See the "Recommended key scope" section of `skills/kraken-setup/SKILL.md`
   before generating a key.
3. Add `account` if you want the agent to read balances and positions.
4. Only enable `funding`, `earn`, `subaccount`, or `all` if you specifically
   need those workflows. Each of these unlocks tools that can move funds off
   the exchange or alter account structure.

Switching to `-s all` expands the set of tools the agent can reach by
prompt-injection or mistake. Prefer a narrow list and widen it deliberately.

## Tool Results

`structuredContent` is always a JSON object. An array or scalar output (for example `kraken_deposit_status`) arrives as `{"result": <output>}`; the text block carries the output as `-o json` prints it. A failed call returns the CLI error envelope: on `"verify_first": true`, check order or transfer state before calling again.

## Safety

Dangerous tools include `[DANGEROUS: requires human confirmation]` in their description and carry the MCP `destructive_hint` annotation. `kraken_workspace_create` with mode `live` is gated the same way. In guarded mode (the default for any service list), the server asks the operator to approve each such call through MCP elicitation. A client without elicitation cannot show that prompt, so the server accepts `acknowledged=true` in the tool arguments instead, and the model writes those arguments: no person is asked unless the client's own tool-approval prompt is on. Use guarded mode with a client that supports elicitation, or keep the client's per-tool approval enabled. In autonomous mode (`--allow-dangerous`), the per-call confirmation is disabled. MCP clients that respect annotations may still prompt at the client layer.

For fully autonomous operation, see `skills/kraken-autonomy-levels/SKILL.md`.

## Gemini CLI Extension

If using Gemini CLI, install the extension directly:

```bash
gemini extensions install https://github.com/krakenfx/kraken-cli
```

The `gemini-extension.json` includes `mcpServers` config, so the MCP server starts automatically.

## Troubleshooting

**"No tools found"**: Check that `kraken-mcp` is on your PATH and the service list is valid. Releases before this one installed `kraken` only: run the one-line installer again. Run `kraken-mcp -s market` manually to verify it starts; it prints one `server_start` line, then waits for a client. Stop it with Ctrl-C.

**Auth errors on tool calls**: Set `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` in the environment where your MCP client runs.

**Too many tools**: Reduce the service list. `kraken-mcp -s market,trade` is a good starting point.

**Streaming commands not available**: WebSocket streaming commands are excluded from MCP v1. Use `kraken ws ...` directly from the terminal for streaming.

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
