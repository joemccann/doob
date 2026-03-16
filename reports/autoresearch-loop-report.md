# Doob Autoresearch Loop Report (Web-Seeded + Backtest-Confirmed)

## Purpose
Create a repeatable loop that generates **net-new, warehouse-compatible** strategy hypotheses, backtests them through `doob`, and ranks candidates by forward-tested signal quality and risk-adjusted performance.

The loop is implemented in:
- `src/bin/autoresearch_loop.rs` (Rust binary `autoresearch_loop`)
- artifacts in `reports/`

## Core idea
The key design is to force a tight discovery-to-confirmation path:
1. Discover novelty from arXiv + related strategy literature using `EXA_API_KEY`.
2. Map each paper theme to a parameterized doob strategy (breadth or drift family).
3. Backtest in walk-forward windows.
4. Score, rank, persist results, and re-run only top candidates.

The resulting hypotheses are still constrained to doob’s local data model (no external price feeds).

## Data/access assumptions in this repo
- doob reads local parquet price data from `~/market-warehouse/...`.
- Universe presets available from doob strategy args (`ndx100`, `sp500`, `r2k`, custom preset, all-stocks).
- All looped candidates use doob-native strategies only:
  - `breadth-washout`
  - `breadth-ma`
  - `breadth-dual-ma`
  - `ndx100-breadth-washout`
  - `overnight-drift`
  - `intraday-drift`

## Strategy families used for discovery

### 1) Breadth regime families
- `breadth-washout` with oversold/overbought and multiple horizon slices.
- `breadth-ma` with variable MA length and threshold.
- `breadth-dual-ma` with short/long MA pullback + threshold logic.
- `ndx100-breadth-washout` wrapper as an additional constrained variant.

Why this maps to data in doob:
- all three families produce `forward_summary` in JSON, so they are directly scoreable.

### 2) Drift execution families
- `overnight-drift` (with/without VIX filter).
- `intraday-drift` (long and short variants).

Why this maps to data in doob:
- both families output structured `results[]` with cagr/sharpe/max_drawdown/var.

### 3) Exa-seeded candidates
- each fetched idea is tagged by keywords (`momentum`, `reversion`, `regime`, `intraday`, `breadth`), then converted into one or more concrete strategy parameter sets.
- this creates net-new candidates without hard-coding only a fixed grid.

## Scoring and ranking
- Breadth families:
  - `2.0*(cumulative_return_pct/100) + 1.4*sharpe - 1.2*|max_drawdown_pct|/100 - 0.25*max(0,var_95_pct/100)`
- Drift families:
  - `2.0*cagr + 1.2*sharpe - 1.8*|max_drawdown| - 0.5*max(0,var_95)`
- Walk-forward score:
  - `0.65 * train_score + 0.35 * test_score`

Gates:
- non-finite metrics filtered out
- minimum observations and minimum trigger count per candidate
- JSON parse must succeed and strategy output schema must match

## How to run the loop

Prereqs:
- build doob (`cargo build --release`)
- ensure `EXA_API_KEY` is loaded in shell.

Run examples:

```bash
  cargo run --release --bin autoresearch_loop -- \
  --seed-web \
  --candidates 60 \
  --top 15 \
  --train-start 2020-01-01 \
  --train-end 2024-12-31 \
  --test-start 2025-01-01 \
  --test-end 2026-03-11 \
  --train-sessions 1008 \
  --test-sessions 252
```

The loop currently derives breadth window sessions from the train/test date span (with CLI fallback if needed), then runs both windows and prints the ranked shortlist.

## Artifacts produced
- `reports/autoresearch-ledger.jsonl` (append-only run ledger)
- `reports/autoresearch-exa-ideas.json` (seed ideas captured when `--seed-web` is used)

To inspect:
- `cat reports/autoresearch-ledger.jsonl`
- `cat reports/autoresearch-exa-ideas.json`

## Practical operating loop (how I would run it in production)
1. Run baseline pass with `--seed-web` and 40–80 candidates.
2. Re-run top 5 candidates with a tighter or more recent test window.
3. Record top strategy IDs and lock only candidates with positive test score and stable behavior.
4. Optionally add the locked candidates to a short, manual rerun set with alternative seeds.
