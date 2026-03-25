# Skills Index

50 agent skills for `kraken-cli`, organized by category. Covers spot and futures paper trading, live trading, and portfolio management.

## Core

Shared runtime contract, safety rules, autonomy progression, and MCP integration.

| Skill | Description |
|-------|-------------|
| [kraken-shared](./kraken-shared/SKILL.md) | Auth, invocation contract, parsing, and safety rules. |
| [kraken-autonomy-levels](./kraken-autonomy-levels/SKILL.md) | Progress from read-only to fully autonomous agent trading. |
| [kraken-mcp-integration](./kraken-mcp-integration/SKILL.md) | Connect MCP clients (Claude Desktop, Gemini CLI, Cursor) to kraken-cli. |
| [kraken-rate-limits](./kraken-rate-limits/SKILL.md) | API rate limit budgets across spot and futures tiers. |
| [kraken-order-types](./kraken-order-types/SKILL.md) | Complete reference for all spot and futures order types and modifiers. |
| [kraken-error-recovery](./kraken-error-recovery/SKILL.md) | Handle order failures, duplicate submissions, and network errors safely. |

## Market Data

Price reads, multi-pair screening, alerts, and streaming.

| Skill | Description |
|-------|-------------|
| [kraken-market-intel](./kraken-market-intel/SKILL.md) | Ticker, orderbook, OHLC, and streaming market reads. |
| [kraken-multi-pair](./kraken-multi-pair/SKILL.md) | Multi-pair screening, watchlists, spread, and volume comparison. |
| [kraken-alert-patterns](./kraken-alert-patterns/SKILL.md) | Price alerts, threshold monitoring, and notification triggers. |
| [kraken-ws-streaming](./kraken-ws-streaming/SKILL.md) | Real-time WebSocket streaming for spot and futures. |

## Spot Trading

Order execution, position management, and spot paper testing.

| Skill | Description |
|-------|-------------|
| [kraken-spot-execution](./kraken-spot-execution/SKILL.md) | Safe spot order execution with validation and confirmation gates. |
| [kraken-stop-take-profit](./kraken-stop-take-profit/SKILL.md) | Stop-loss and take-profit management for risk-bounded positions. |
| [kraken-fee-optimization](./kraken-fee-optimization/SKILL.md) | Minimize fees through maker orders, volume tiers, and fee currency selection. |
| [kraken-paper-strategy](./kraken-paper-strategy/SKILL.md) | Test spot strategies on paper trading before live deployment. |
| [kraken-paper-to-live](./kraken-paper-to-live/SKILL.md) | Promote validated paper strategies (spot and futures) to live trading. |
| [kraken-risk-operations](./kraken-risk-operations/SKILL.md) | Operational risk controls for live agent trading sessions. |

## Futures

Futures trading, risk management, paper testing, and advanced strategies.

| Skill | Description |
|-------|-------------|
| [kraken-futures-trading](./kraken-futures-trading/SKILL.md) | Futures order lifecycle: place, edit, cancel, batch, positions. Includes paper trading. |
| [kraken-futures-risk](./kraken-futures-risk/SKILL.md) | Leverage, funding rates, margin health, and liquidation awareness. |
| [kraken-liquidation-guard](./kraken-liquidation-guard/SKILL.md) | Prevent futures liquidation through margin monitoring and emergency procedures. |
| [kraken-basis-trading](./kraken-basis-trading/SKILL.md) | Delta-neutral spot-futures basis trades. |
| [kraken-funding-carry](./kraken-funding-carry/SKILL.md) | Earn funding rate payments with hedged carry positions. |

## Trading Strategies

Systematic trading patterns for agent automation.

| Skill | Description |
|-------|-------------|
| [kraken-dca-strategy](./kraken-dca-strategy/SKILL.md) | Dollar cost averaging with scheduled buys and cost tracking. |
| [kraken-grid-trading](./kraken-grid-trading/SKILL.md) | Grid trading with layered orders across a price range. |
| [kraken-rebalancing](./kraken-rebalancing/SKILL.md) | Portfolio rebalancing to maintain target allocations. |
| [kraken-twap-execution](./kraken-twap-execution/SKILL.md) | Time-weighted average price execution for large orders. |

## Funding & Earn

Deposits, withdrawals, staking, and fund management.

| Skill | Description |
|-------|-------------|
| [kraken-funding-ops](./kraken-funding-ops/SKILL.md) | Deposits, withdrawals, and wallet transfers. |
| [kraken-earn-staking](./kraken-earn-staking/SKILL.md) | Earn strategies, staking allocation, and deallocation. |
| [kraken-tax-export](./kraken-tax-export/SKILL.md) | Export trade history, ledgers, and cost basis data for tax reporting. |

## Portfolio & Account

Balance analysis, P&L tracking, subaccounts, and exports.

| Skill | Description |
|-------|-------------|
| [kraken-portfolio-intel](./kraken-portfolio-intel/SKILL.md) | Balance analysis, P&L tracking, trade history, and exports. |
| [kraken-subaccount-ops](./kraken-subaccount-ops/SKILL.md) | Subaccount creation, transfers, and strategy isolation. |

## Recipes

Multi-step workflows combining multiple skills.

### Strategy Recipes

| Skill | Description |
|-------|-------------|
| [recipe-start-dca-bot](./recipe-start-dca-bot/SKILL.md) | Set up and run a DCA bot from paper test to live. |
| [recipe-launch-grid-bot](./recipe-launch-grid-bot/SKILL.md) | Deploy a grid trading bot with paper validation and live safety controls. |
| [recipe-trailing-stop-runner](./recipe-trailing-stop-runner/SKILL.md) | Ride a trend with a trailing stop that locks in profits on reversal. |
| [recipe-basis-trade-entry](./recipe-basis-trade-entry/SKILL.md) | Enter a spot-futures basis trade when the premium exceeds a threshold. |
| [recipe-futures-hedge-spot](./recipe-futures-hedge-spot/SKILL.md) | Hedge a spot holding with a short futures position. |
| [recipe-funding-rate-scan](./recipe-funding-rate-scan/SKILL.md) | Scan perpetual contracts for attractive funding rate carry opportunities. |
| [recipe-paper-strategy-backtest](./recipe-paper-strategy-backtest/SKILL.md) | Backtest a strategy using paper trading across multiple sessions. |

### Portfolio Recipes

| Skill | Description |
|-------|-------------|
| [recipe-weekly-rebalance](./recipe-weekly-rebalance/SKILL.md) | Weekly portfolio rebalance to maintain target allocations. |
| [recipe-daily-pnl-report](./recipe-daily-pnl-report/SKILL.md) | Daily profit and loss summary from trades and balances. |
| [recipe-portfolio-snapshot-csv](./recipe-portfolio-snapshot-csv/SKILL.md) | Export a portfolio snapshot with balances and valuations to CSV. |
| [recipe-subaccount-capital-rotation](./recipe-subaccount-capital-rotation/SKILL.md) | Rotate capital between subaccounts based on strategy performance. |
| [recipe-fee-tier-progress](./recipe-fee-tier-progress/SKILL.md) | Track 30-day volume progress toward the next fee tier. |

### Market Data Recipes

| Skill | Description |
|-------|-------------|
| [recipe-morning-market-brief](./recipe-morning-market-brief/SKILL.md) | Morning summary with prices, volume, and portfolio state. |
| [recipe-multi-pair-breakout-watch](./recipe-multi-pair-breakout-watch/SKILL.md) | Monitor multiple pairs for price breakouts from defined ranges. |
| [recipe-track-orderbook-depth](./recipe-track-orderbook-depth/SKILL.md) | Monitor order book depth and bid-ask imbalance for liquidity signals. |
| [recipe-price-level-alerts](./recipe-price-level-alerts/SKILL.md) | Set up price level alerts that notify when key levels are crossed. |

### Risk Recipes

| Skill | Description |
|-------|-------------|
| [recipe-emergency-flatten](./recipe-emergency-flatten/SKILL.md) | Cancel all orders and close all positions in an emergency. |
| [recipe-drawdown-circuit-breaker](./recipe-drawdown-circuit-breaker/SKILL.md) | Stop trading when portfolio drawdown exceeds a threshold. |

### Funding Recipes

| Skill | Description |
|-------|-------------|
| [recipe-withdrawal-to-cold-storage](./recipe-withdrawal-to-cold-storage/SKILL.md) | Safely withdraw funds to a pre-approved cold storage address. |
| [recipe-earn-yield-compare](./recipe-earn-yield-compare/SKILL.md) | Compare earn strategy yields across assets to find the best rate. |
