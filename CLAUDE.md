# doob — Quantitative Strategy Research & Backtesting

Rust binary for quantitative strategy research. Reads from the shared `~/market-warehouse/` data lake (bronze parquet) but has zero dependency on the `market-data-warehouse` repo.

## Crate Layout

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

## Data Contract

Reads from `~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet`.

Expected columns: `trade_date`, `open`, `high`, `low`, `close`, `volume`.

**Important**: `adj_close == close` in this warehouse (IB TRADES data is split-adjusted but not dividend-adjusted). Buy-and-hold CAGR will understate true total return by ~1.3%/yr due to missing dividends.

## Config Precedence

`DOOB_WAREHOUSE_PATH` env var → `.env` file → `~/market-warehouse` (default)

## Strategy Catalog

```bash
doob run overnight-drift --no-vix-filter --no-plots
doob run intraday-drift --ticker SPY --short
doob run breadth-washout --universe ndx100 --signal-mode oversold
doob run ndx100-sma-breadth --end-date 2026-03-11
doob list-strategies
doob list-presets
```

## JSON Output (for AI agents / programmatic use)

All strategies support `--output json` for structured machine-readable output:

```bash
doob --output json run overnight-drift --no-vix-filter
doob run intraday-drift --ticker SPY --output json
```

The flag is global and can appear before or after the subcommand. When active, all human-readable text and progress messages are suppressed — only a single JSON object is written to stdout.

## Building & Testing

```bash
cargo build --release
cargo test
```

### Test Rules

1. 120 unit tests covering all modules
2. Use `tempfile` crate for temporary directories in tests
3. Mock all external I/O (file paths, network requests)
4. Tests run in < 0.1s (no real data dependencies)

## Key Dependencies

- `polars` — DataFrame & parquet I/O
- `nalgebra` — Linear algebra (ADF test OLS)
- `clap` — CLI argument parsing (derive), global `--output` flag
- `serde` + `serde_json` — JSON serialization for `--output json`
- `reqwest` — HTTP client (VIX download, Yahoo Finance, NASDAQ API)
- `chrono` — Date/time operations
- `rayon` — Parallel data fetching

## How to Add a New Strategy

1. Create `src/strategies/my_strategy.rs` with a `run(args, fmt)` function
2. Define `MyStrategyArgs` using clap derive
3. Add the strategy to `StrategyCommand` enum in `src/cli.rs`
4. Wire it up in `src/main.rs` match arm
5. Write tests in the `#[cfg(test)] mod tests` block
6. Run `cargo test`
