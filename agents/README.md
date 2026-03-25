# Agent Toolkit

Machine-readable contracts for building AI agents on top of `kraken-cli`.

## Source of Truth

`tool-catalog.json` is the canonical command contract.

- Coverage: **151 commands** with parameters, types, defaults, and examples
- Every command includes: group, description, auth requirement, `dangerous` flag, and parameter schemas
- 34 commands are marked `dangerous: true` (orders, withdrawals, transfers, cancel-all, staking)

`error-catalog.json` is the error routing contract.

- 9 error categories with retry/backoff guidance

## Contents

| File | Purpose |
|------|---------|
| `tool-catalog.json` | Canonical command catalog (151 commands) |
| `error-catalog.json` | Error categories with retry guidance |
| `examples/` | Runnable shell examples |

## Using the Catalog

### Load commands in Python

```python
import json

with open("agents/tool-catalog.json") as f:
    catalog = json.load(f)

for cmd in catalog["commands"]:
    print(f"{cmd['group']}/{cmd['name']}: {cmd['description']}")
    if cmd.get("dangerous"):
        print("  ⚠ requires human confirmation")
```

### Build tool definitions for any platform

The catalog contains everything needed to generate tool schemas for OpenAI, Anthropic, MCP, LangChain, or any other framework. Each command entry includes:

```json
{
  "name": "order-buy",
  "group": "trade",
  "command": "kraken order buy <PAIR> <VOLUME>",
  "description": "Place a buy order.",
  "auth_required": true,
  "dangerous": true,
  "parameters": [
    { "name": "PAIR", "type": "string", "required": true, "positional": true },
    { "name": "--type", "type": "string", "required": false }
  ],
  "example": "kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate"
}
```

### Filter by safety

```python
safe_commands = [c for c in catalog["commands"] if not c.get("dangerous")]
dangerous_commands = [c for c in catalog["commands"] if c.get("dangerous")]
```

## Related Files

- `../CONTEXT.md`: Runtime context for tool-using agents
- `../AGENTS.md`: Full integration guide
- `../skills/`: Goal-oriented workflow skills
- `../gemini-extension.json`: Gemini CLI extension manifest
