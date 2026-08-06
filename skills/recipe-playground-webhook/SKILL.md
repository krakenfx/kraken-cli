---
name: recipe-playground-webhook
version: 1.0.0
description: "Example driver: turn incoming webhook signals into validated paper trades inside a recorded session."
metadata:
  openclaw:
    category: "recipe"
    domain: "sessions"
  requires:
    bins: ["kraken", "python3"]
    skills: ["kraken-playground"]
---

# Playground: Webhook → Paper Trade

> **PREREQUISITE:** Load `kraken-playground` to run this recipe.
> **This recipe is an example, not a boundary.** Adapt the steps to your signal source, or write your own driver (see `kraken-playground` → Driving Your Own Hypothesis).

Bridge an external system into the sandbox: a TradingView alert, a CI job, or a plain `curl` POSTs a signal to a local listener, and each valid signal becomes a paper order inside a recorded session. The market tape, the fills, and the signal-to-order decisions all land in one window for replay and P&L.

Use this skill for:
- wiring an existing alerting system to paper execution without touching its config
- testing a signal feed's quality (how its entries score) before any real money
- recording an event-driven loop whose events come from outside the market

## Important

This recipe records a session. It never places a live order.

The webhook payload is **untrusted input**. Every field is validated against an allowlist or a numeric bound before it reaches an order — a signal that fails any check is logged as a skip, never executed, and never interpolated into a command.

The listener binds `127.0.0.1` only. Do not bind other interfaces and do not port-forward it; anyone who can reach the port can trade the paper account. If the signal source is remote, tunnel (e.g. `ssh -R`) rather than expose.

The signal stream is the loop, not `/loop` — the same long-lived-process exception as the dip-triggered recipe (see `kraken-playground` → Pacing).

## Signal Contract

One JSON object per POST body:

```json
{"symbol": "BTC/USD", "side": "buy", "qty": "0.001", "token": "<shared-secret>", "note": "free text"}
```

`symbol`, `side`, `qty` are required; `token` is required when the `token` param is set; `note` is echoed into the decision reason.

## Params

Every bound that gates execution goes in `--strategy-params`:

- `port`: listener port on 127.0.0.1
- `allowed_symbols`: the only pairs a signal may trade — must be a subset of the session's recorded `--symbols`, or fills have no marks to score by
- `max_notional_per_signal`: quote-currency cap per order; a signal above it is skipped, not clamped
- `max_signals`: stop after this many executed signals
- `token`: optional shared secret; when set, signals without a matching `token` field are dropped

## Quick Start

Natural language:

```
Listen on localhost:8787 for webhook signals. Trade only BTC/USD, at most
$500 per signal, at most 10 signals, then stop. Record every accepted and
rejected signal with its reason. At the end, show P&L.
```

## Start the Session

Work inside a paper workspace (create one once: `kraken workspace create hook --capital 10000 --mode paper`), then:

```bash
export KRAKEN_WORKSPACE=hook

kraken session start \
  --symbols BTC/USD --channels ticker,trade --to duckdb,jsonl \
  --label hook-btc-$(date +%Y%m%d-%H%M%S) \
  --strategy recipe-playground-webhook \
  --strategy-params '{"port":8787,"allowed_symbols":["BTC/USD"],"max_notional_per_signal":500,"max_signals":10}' \
  -o json 2>/dev/null &

# The session_started stdout line carries the id: {"type":"session_started","session":"s<n>",...}
```

Print the session id to the user right after starting, and again in the final report.

## Listen and Decide

The listener appends each POST body to a signals file **outside the session directory** (the recorder owns that tree); the decision loop tails it. Both die with the session.

```bash
PORT=8787
ALLOWED_SYMBOLS="BTC/USD"          # space-separated allowlist
MAX_NOTIONAL=500
MAX_SIGNALS=10
SIGNALS=$(mktemp -t hook-signals)

python3 - "$PORT" "$SIGNALS" <<'PY' &
import http.server, sys
port, path = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        with open(path, "ab") as f:
            f.write(body.strip() + b"\n")
        self.send_response(204); self.end_headers()
    def log_message(self, *args): pass
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
PY
LISTENER=$!

DONE=0
tail -n +1 -f "$SIGNALS" | while read -r sig; do
  SYMBOL=$(echo "$sig" | jq -r '.symbol // empty')
  SIDE=$(echo "$sig" | jq -r '.side // empty')
  QTY=$(echo "$sig" | jq -r '.qty // empty')
  NOTE=$(echo "$sig" | jq -r '.note // ""')

  # Untrusted input: allowlist the enum and the pair, bound the number.
  # A failed check is a recorded skip — never a clamped or guessed order.
  case "$SIDE" in buy|sell) ;; *)
    kraken session note --kind skip --reason "webhook rejected: bad side '$SIDE'" -o json 2>/dev/null; continue;;
  esac
  case " $ALLOWED_SYMBOLS " in *" $SYMBOL "*) ;; *)
    kraken session note --kind skip --reason "webhook rejected: symbol '$SYMBOL' not in allowlist" -o json 2>/dev/null; continue;;
  esac
  if ! echo "$QTY" | grep -qE '^[0-9]*\.?[0-9]+$'; then
    kraken session note --kind skip --reason "webhook rejected: non-numeric qty '$QTY'" -o json 2>/dev/null; continue
  fi
  PRICE=$(kraken ticker "$SYMBOL" -o json 2>/dev/null | jq -r '.. | .last? // empty' | head -1)
  NOTIONAL=$(echo "scale=2; $QTY * $PRICE" | bc -l)
  if (( $(echo "$NOTIONAL > $MAX_NOTIONAL" | bc -l) )); then
    kraken session note --kind skip --symbol "$SYMBOL" \
      --reason "webhook rejected: notional \$$NOTIONAL exceeds cap \$$MAX_NOTIONAL" -o json 2>/dev/null; continue
  fi

  kraken order "$SIDE" "$SYMBOL" "$QTY" --type market \
    --reason "webhook signal: $SIDE $QTY $SYMBOL (~\$$NOTIONAL) — $NOTE" -o json 2>/dev/null
  DONE=$((DONE + 1))
  [ "$DONE" -ge "$MAX_SIGNALS" ] && break
done

kill "$LISTENER" 2>/dev/null
kraken session stop -o json 2>/dev/null
```

Send a test signal:

```bash
curl -s -X POST "127.0.0.1:8787" -d '{"symbol":"BTC/USD","side":"buy","qty":"0.001","note":"smoke test"}'
```

## Stop and Review

```bash
kraken session show -o json 2>/dev/null | jq '.summary'

kraken session decisions --session s<n> -o json 2>/dev/null \
  | jq -c '.decisions[] | {kind, symbol, reason}'
```

Report:
- signals received vs executed vs rejected, with the rejection reasons
- fill quality: reason-joined entries from `kraken explain pnl`
- P&L, and whether the signal source earns a bigger cap

## Hard Rules

- This recipe records a session. It never places a live order.
- The listener binds `127.0.0.1` only — never another interface, never port-forwarded.
- Webhook payloads are untrusted: enum fields pass an allowlist, numbers pass a bound, and nothing from the payload is ever executed or eval'd — rejected signals become `skip` notes, not orders.
- `allowed_symbols` stays a subset of the session's recorded `--symbols`.
- The signals file lives outside the session directory; the CLI recorder stays the single writer of the session's artifacts.
- Cap executed signals with `max_signals`; a runaway signal source must exhaust the cap, not the account.
- Keep every gating number in `--strategy-params` and the arithmetic in `jq`/`bc`.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
