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

## Autoresearch Loop (Rust)

Build and run the automated candidate-discovery loop from Rust:

```bash
# build binaries
cargo build --release

# run with arXiv/Exa web seeding and web-driven net-new candidate exploration
# defaults: --seed-web --candidates 100 --top 10 --verbose --sessions 1008/252
cargo run --release --bin autoresearch_loop -- --seed-web --verbose

# set explicit run envelope and ranking depth
cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose

# run a larger search (or shorter top list) as needed
cargo run --release --bin autoresearch_loop -- --seed-web --candidates 200 --top 15 --verbose
```

Optional settings:

- `--train-start`, `--train-end`, `--test-start`, `--test-end`
- `--train-sessions`, `--test-sessions`
- `--doob-bin target/release/doob`
- `--random-seed`
- paper-research loop uses `paper-research` strategy only (no built-in breadth/overnight baseline strategies are queued by default)
- `--seed-web` uses Exa/arXiv discovery and then generates deterministic mutations around web-proposed ideas

Output artifacts:

- `reports/autoresearch-top10-interactive-report.html` (interactive top 10 browser report)
- `reports/autoresearch-ledger.jsonl` (full candidate ledger)
- `reports/autoresearch-exa-ideas.json` (web-seed scrape and normalization)

Output is now table-formatted and includes:

- candidate ID + strategy + strategy category
- focused assets, horizon, source, and rationale
- train/test scores and summary stats
- machine-copyable command args for the best candidate

To provide the Exa key used for web seeding:

```bash
cp .env.example .env
# edit .env: EXA_API_KEY=your_exa_api_key_here
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

## Output Formats

All strategies support `--output text` (default), `--output json`, and `--output md`:

```bash
doob --output json run overnight-drift --no-vix-filter
doob --output md run breadth-washout --universe ndx100
doob run intraday-drift --ticker SPY --output json
```

## Testing

```bash
# Unit tests (146 tests)
cargo test

# CLI integration tests (106 tests, requires ~/market-warehouse)
./tests/cli_integration.sh
```

## License

MIT
