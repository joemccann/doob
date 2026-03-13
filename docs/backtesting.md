# Quant Backtesting Framework in Python

Repo note:
- The live breadth strategy entry point in this package is `doob.strategies.breadth_washout`.
- It supports `oversold` and `overbought` trigger modes across named universes, custom presets, explicit ticker lists, and `all-stocks`.
- Official point-in-time membership is currently implemented only for `ndx100`; the other supported universes run as static baskets.
- Data is read from the shared `~/market-warehouse/` bronze parquet layer.

See the full backtesting framework reference in the parent market-data-warehouse repo's `docs/backtesting.md`.
