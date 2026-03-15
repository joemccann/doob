# doob

![doob](.github/hero.png)

Quantitative strategy research and backtesting CLI. Reads all price data from local `~/market-warehouse/` parquet files — no external API calls for price data.

## Prerequisites

- Rust 2024 edition (1.85+)
- Populated `~/market-warehouse/` data lake (bronze parquet layer) — see [market-data-warehouse](https://github.com/joemccann/market-data-warehouse)

## Install

Build and install to `~/.cargo/bin` (must be in your `$PATH`):

```bash
cargo build --release
cp target/release/doob ~/.cargo/bin/doob
```

Verify:

```bash
doob list-strategies
```

## Update

After pulling changes or making edits, rebuild and reinstall:

```bash
cargo build --release && cp target/release/doob ~/.cargo/bin/doob
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
doob run breadth-ma --universe ndx100 --short-period 50 --threshold 80
doob run breadth-dual-ma --universe ndx100 --short-period 50 --long-period 200 --threshold 20
doob run ndx100-sma-breadth --end-date 2026-03-11
```

## Strategy Catalog

| Strategy | Description |
|----------|-------------|
| `overnight-drift` | Buy SPY at close, sell at next open. Optional VIX regime filter. |
| `intraday-drift` | Buy at open, sell at close same day. Supports long/short. |
| `breadth-washout` | Generic breadth signal across any universe (ndx100, sp500, r2k, all-stocks). |
| `breadth-ma` | Single MA breadth (default 50-day). % below/above N-day MA. |
| `breadth-dual-ma` | Dual MA breadth. Identifies pullbacks within uptrends. |
| `ndx100-sma-breadth` | NASDAQ-100 5-day SMA breadth analysis with forward returns. |

## Data Architecture

All price data is read from local warehouse parquet files. No Yahoo Finance or external price APIs.

- **Price data**: `~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet`
- **Universe membership**: `presets/<universe>.json` (e.g. `ndx100.json`, `sp500.json`)
- **VIX data**: CBOE CSV, cached locally for 24h (only external HTTP call)

A 10-year, 101-ticker backtest with full risk metrics runs in **~0.3 seconds**.

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
