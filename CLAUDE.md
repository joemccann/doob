# doob — Quantitative Strategy Research & Backtesting

Standalone Python package for quantitative strategy research. Reads from the shared `~/market-warehouse/` data lake (bronze parquet) but has zero dependency on the `market-data-warehouse` repo.

## Package Layout

```
src/doob/
├── config.py              # Centralized config: warehouse path, output root, presets dir
├── data/
│   ├── paths.py           # Parquet path resolution helpers
│   ├── discovery.py       # Symbol discovery from bronze layer
│   ├── readers.py         # Parquet/DuckDB data loaders, CBOE VIX cache
│   └── presets.py         # Preset loading + validation
├── metrics/
│   ├── performance.py     # cagr, sharpe, max_drawdown, var_95, annual_returns_table
│   └── fees.py            # IBKR fee model constants + ibkr_roundtrip_cost()
├── strategies/
│   ├── overnight_drift.py # Buy close, sell next open; optional VIX filter
│   ├── intraday_drift.py  # Buy open, sell close same day; long or short
│   ├── breadth_washout.py # Generic breadth signal across any universe
│   ├── ndx100_sma_breadth.py  # NDX-100 SMA breadth analysis + forward returns
│   └── ndx100_breadth_washout.py  # Thin wrapper
└── cli.py                 # Unified entrypoint: python -m doob run <strategy>
```

## Data Contract

Reads from `~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet`.

Expected columns: `trade_date`, `open`, `high`, `low`, `close`, `volume`.

**Important**: `adj_close == close` in this warehouse (IB TRADES data is split-adjusted but not dividend-adjusted). Buy-and-hold CAGR will understate true total return by ~1.3%/yr due to missing dividends.

## Config Precedence

`DOOB_WAREHOUSE_PATH` env var → `.env` file → `~/market-warehouse` (default)

## Strategy Catalog

```bash
python -m doob run overnight-drift --no-vix-filter --no-plots
python -m doob run intraday-drift --ticker SPY --short
python -m doob run breadth-washout --universe ndx100 --signal-mode oversold
python -m doob run ndx100-sma-breadth --end-date 2026-03-11
python -m doob list-strategies
python -m doob list-presets
```

## Testing

```bash
source ~/market-warehouse/.venv/bin/activate  # or use the doob venv
pip install -e ".[all]"
python -m pytest tests/ -v --cov=doob --cov-report=term-missing
```

### Rules

1. Coverage target: 95% (`pyproject.toml` enforces `fail_under = 95`)
2. Mock all external I/O (file paths, network requests)
3. Use `tmp_path` / `tmp_warehouse` fixtures for file-based tests
4. Mark tests needing real `~/market-warehouse/` with `@pytest.mark.integration`
5. `if __name__ == "__main__"` blocks are excluded from coverage

## How to Add a New Strategy

1. Create `src/doob/strategies/my_strategy.py` with a `main()` function using `argparse`
2. Import shared modules: `from doob.metrics.performance import cagr, sharpe, ...`
3. Add the strategy to `STRATEGY_MAP` in `src/doob/cli.py`
4. Write tests in `tests/test_my_strategy.py`
5. Run `python -m pytest tests/ -v --cov=doob`
