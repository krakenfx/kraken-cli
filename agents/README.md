# Agent Toolkit

Machine-readable contracts for building AI agents on top of `kraken-cli`.

## Source of Truth

`tool-catalog.json` is the canonical command contract.

- Coverage: **188 commands**, each callable one carrying a JSON Schema generated
  from its own Rust type
- Every command includes: group, description, auth requirement, `dangerous`
  flag, and parameter schemas
- 43 commands are marked `dangerous: true` (orders, withdrawals, transfers,
  cancel-all, staking, account and margin settings)

`error-catalog.json` is the error routing contract.

- 10 error categories with retry/backoff guidance

## Contents

| File                 | Purpose                                             |
| -------------------- | --------------------------------------------------- |
| `tool-catalog.json`  | Canonical command catalog (188 commands), generated |
| `error-catalog.json` | Error categories with retry guidance                |
| `examples/`          | Runnable shell examples                             |

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

The catalog contains everything needed to generate tool schemas for OpenAI,
Anthropic, MCP, LangChain, or any other framework. Each command entry includes:

```json
{
  "name": "order-buy",
  "group": "trade",
  "command": "kraken order buy",
  "usage": "kraken order buy <PAIR> <VOLUME>",
  "tool": "kraken_order_buy",
  "reach": "tool",
  "description": "Place a buy order.\n\nRouted by workspace mode: …",
  "credentials": "spot",
  "dangerous": true,
  "paper_safe": true,
  "effect": "paper_safe",
  "input_schema": {
    "type": "object",
    "properties": { "pair": { "type": "string" } }
  },
  "example": "kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate"
}
```

`input_schema` is a JSON Schema generated from the command's own Rust type, so
it cannot drift from what the command accepts. `constraints` appears on the
commands whose arguments constrain each other (`ticker` takes `pairs` or
`--all`, never both) — no schema keyword states that portably, and the MCP seam
enforces it.

```python
# Safety filter: `credentials` replaced the old `auth_required` boolean in
# schema_version 2.0.0 and says *which* key is needed.
public = [c for c in catalog["commands"] if c["credentials"] == "none"]
```

### Filter by safety

```python
# `local_or_live` commands reach the real account when one argument is set, so
# check their arguments on every call instead.
safe_commands = [
    c for c in catalog["commands"]
    if not c.get("dangerous") and c["effect"] != "local_or_live"
]
dangerous_commands = [c for c in catalog["commands"] if c.get("dangerous")]
```

## Related Files

- `../CONTEXT.md`: Runtime context for tool-using agents
- `../AGENTS.md`: Full integration guide
- `../skills/`: Goal-oriented workflow skills
- `../gemini-extension.json`: Gemini CLI extension manifest
