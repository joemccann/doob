# doob

Quantitative strategy research and backtesting CLI. Reads from the shared `~/market-warehouse/` data lake.

## Prerequisites

- Rust 2024 edition (1.85+)
- Populated `~/market-warehouse/` data lake (bronze parquet layer) — see [market-data-warehouse](https://github.com/joemccann/market-data-warehouse)

## Install

```bash
cargo build --release
# Binary at ./target/release/doob
```

## Quick Start

```bash
# List available strategies and presets
doob list-strategies
doob list-presets

# Run strategies
doob run overnight-drift --no-plots
doob run intraday-drift --ticker SPY
doob run intraday-drift --ticker SPY --short
doob run breadth-washout --universe ndx100 --signal-mode oversold
doob run ndx100-sma-breadth --end-date 2026-03-11
```

## Strategy Catalog

| Strategy | Description |
|----------|-------------|
| `overnight-drift` | Buy SPY at close, sell at next open. Optional VIX regime filter. |
| `intraday-drift` | Buy at open, sell at close same day. Supports long/short. |
| `breadth-washout` | Generic breadth signal across any universe (ndx100, sp500, r2k, all-stocks). |
| `ndx100-sma-breadth` | NASDAQ-100 5-day SMA breadth analysis with forward returns. |

## JSON Output

For AI agents or programmatic consumption, pass `--output json` to get structured output:

```bash
doob --output json run overnight-drift --no-vix-filter
doob run intraday-drift --ticker SPY --output json
```

## Testing

```bash
cargo test
```

## License

MIT
