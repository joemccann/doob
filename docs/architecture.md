# Architecture

## Why the split

The `market-data-warehouse` repo handles data ingestion, storage, and warehouse management. The `doob` crate is the standalone home for all quantitative strategy research and backtesting. It reads from the shared `~/market-warehouse/` data lake but has zero dependency on the warehouse repo.

## Data contract

doob reads from the bronze parquet layer:

```
~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet
```

Expected schema per parquet file:
- `trade_date` (date/datetime)
- `open`, `high`, `low`, `close` (float)
- `volume` (int)
- `adj_close` (float) — equal to `close` in this warehouse (IB TRADES data, split-adjusted but not dividend-adjusted)

## Crate layout

```
src/
├── main.rs                          # Binary entrypoint
├── lib.rs                           # Library root (re-exports all modules)
├── cli.rs                           # Unified CLI: doob run <strategy> | list-strategies | list-presets
├── config.rs                        # Centralized config: warehouse path, output root, presets dir
├── data/
│   ├── mod.rs
│   ├── paths.rs                     # Parquet path resolution helpers
│   ├── discovery.rs                 # Symbol discovery from bronze layer
│   ├── readers.rs                   # Parquet (polars) data loaders, CBOE VIX cache
│   └── presets.rs                   # Preset loading + validation
├── metrics/
│   ├── mod.rs
│   ├── performance.rs               # cagr, sharpe, max_drawdown, var_95, annual_returns_table
│   └── fees.rs                      # IBKR fee model constants + ibkr_roundtrip_cost()
└── strategies/
    ├── mod.rs
    ├── common.rs                    # Shared: daily_returns, buy_and_hold_equity, formatting, JSON output
    ├── overnight_drift.rs           # Buy close, sell next open; optional VIX filter + ADF test
    ├── intraday_drift.rs            # Buy open, sell close same day; long or short
    ├── breadth_washout.rs           # Generic breadth signal across any universe
    ├── ndx100_sma_breadth.rs        # NDX-100 SMA breadth analysis + forward returns
    └── ndx100_breadth_washout.rs    # Thin wrapper
```

## Key dependencies

- `polars` — DataFrame & parquet I/O
- `nalgebra` — Linear algebra for ADF test OLS
- `clap` — CLI argument parsing with derive macros, global `--output` flag
- `serde` + `serde_json` — JSON serialization for `--output json` mode
- `reqwest` — HTTP client for VIX download, Yahoo Finance, NASDAQ API
- `chrono` — Date/time operations
- `rayon` — Parallel data fetching

## Config precedence

```
DOOB_WAREHOUSE_PATH env var → .env file → ~/market-warehouse (default)
```

## Output modes

All strategies support `--output text` (default) and `--output json`. The flag is global on the `Cli` struct and passed to each strategy's `run(args, fmt)` function. When `json`, strategies emit a single JSON object to stdout with no progress text.

## How to add a new strategy

1. Create `src/strategies/my_strategy.rs`
2. Define `MyStrategyArgs` struct using clap derive
3. Implement `pub fn run(args: &MyStrategyArgs, fmt: OutputFormat) -> Result<()>`
4. Add the strategy to `StrategyCommand` enum in `src/cli.rs`
5. Wire it up in `src/main.rs` match arm
6. Write tests in the `#[cfg(test)] mod tests` block
7. Run `cargo test`
