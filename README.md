# doob

Quantitative strategy research and backtesting package. Reads from the shared `~/market-warehouse/` data lake.

## Prerequisites

- Python 3.12+
- Populated `~/market-warehouse/` data lake (bronze parquet layer) — see [market-data-warehouse](https://github.com/joemccann/market-data-warehouse)

## Install

```bash
pip install -e ".[all]"
```

## Quick Start

```bash
# List available strategies and presets
python -m doob list-strategies
python -m doob list-presets

# Run strategies
python -m doob run overnight-drift --no-plots
python -m doob run intraday-drift --ticker SPY
python -m doob run breadth-washout --universe ndx100 --signal-mode oversold
python -m doob run ndx100-sma-breadth --end-date 2026-03-11
```

## Strategy Catalog

| Strategy | Description |
|----------|-------------|
| `overnight-drift` | Buy SPY at close, sell at next open. Optional VIX regime filter. |
| `intraday-drift` | Buy at open, sell at close same day. Supports long/short. |
| `breadth-washout` | Generic breadth signal across any universe (ndx100, sp500, r2k, all-stocks). |
| `ndx100-sma-breadth` | NASDAQ-100 5-day SMA breadth analysis with forward returns. |

## Testing

```bash
python -m pytest tests/ -v --cov=doob --cov-report=term-missing
```
