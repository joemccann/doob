# Quant Backtesting Framework in Rust

Repo note:
- The live breadth strategy entry point is `doob run breadth-washout`.
- It supports `oversold` and `overbought` trigger modes across named universes, custom presets, explicit ticker lists, and `all-stocks`.
- Official point-in-time membership is currently implemented only for `ndx100`; the other supported universes run as static baskets.
- Data is read from the shared `~/market-warehouse/` bronze parquet layer.
- ADF test uses pure Rust nalgebra OLS with AIC lag selection (no external stats dependency).
- Fee model: IBKR tiered with per-share commission, exchange/regulatory fees, min order, and max cap.
- All strategies support `--output json` for programmatic/agent consumption (single JSON object to stdout, no progress text).

See the full backtesting framework reference in the parent market-data-warehouse repo's `docs/backtesting.md`.
