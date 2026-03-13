# Architecture

## Why the split

The `market-data-warehouse` repo handles data ingestion, storage, and warehouse management. The `doob` package is the standalone home for all quantitative strategy research and backtesting. It reads from the shared `~/market-warehouse/` data lake but has zero dependency on the warehouse repo.

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

## Package layout

```
src/doob/
├── __init__.py          # Package root
├── config.py            # Centralized config (warehouse path, output/presets dirs)
├── data/
│   ├── paths.py         # Parquet path resolution
│   ├── discovery.py     # Symbol discovery from bronze layer
│   ├── readers.py       # Parquet/DuckDB data loaders, CBOE VIX
│   └── presets.py       # Preset loading + validation
├── metrics/
│   ├── performance.py   # cagr, sharpe, max_drawdown, var_95, annual_returns_table
│   └── fees.py          # IBKR fee model
├── strategies/
│   ├── overnight_drift.py
│   ├── intraday_drift.py
│   ├── breadth_washout.py
│   ├── ndx100_sma_breadth.py
│   └── ndx100_breadth_washout.py
└── cli.py               # Unified entrypoint
```

## Config precedence

```
DOOB_WAREHOUSE_PATH env var → .env file → ~/market-warehouse (default)
```

## How to add a new strategy

1. Create `src/doob/strategies/my_strategy.py`
2. Implement a `main()` function that uses `argparse`
3. Import shared modules: `from doob.metrics.performance import cagr, sharpe, ...`
4. Add the strategy to `STRATEGY_MAP` in `src/doob/cli.py`
5. Write tests in `tests/test_my_strategy.py`
6. Run `python -m pytest tests/ -v --cov=doob`
