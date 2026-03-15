# doob AGENTS (Codex Working Instructions)

## Scope

This file is the codex-specific execution guide for this repository.
It complements `CLAUDE.md` with mandatory constraints for automated work.

## Core rule

- Autoresearch loops MUST be implemented and executed in Rust.
- Do not use Python for looping or orchestration of candidate generation, backtest runs, scoring, or ranking.

## Doob architecture at a glance

- Binary strategy runner: `doob` (`src/main.rs`, `src/lib.rs`)
- Core CLI: `src/cli.rs`
- Core data pipeline: `src/data/*`
- Strategy implementations: `src/strategies/*`
- Metrics: `src/metrics/*`
- Autoresearch loop binary: `src/bin/autoresearch_loop.rs`

## Rust autoresearch command

- Build:
  - `cargo build --release`
- Run:
  - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 60 --top 15`
  - verbose:
    - `cargo run --release --bin autoresearch_loop -- --seed-web --verbose`
  - Optional dates/sessions override:
    - `--train-start 2020-01-01 --train-end 2024-12-31 --test-start 2025-01-01 --test-end 2026-03-11 --train-sessions 1008 --test-sessions 252`
  - Optional binary override:
    - `--doob-bin target/release/doob`

## Data and candidate constraints

- Backtests use local warehouse parquet data only.
- Candidate execution must stay within doob-native strategies:
  - `breadth-washout`
  - `breadth-ma`
  - `breadth-dual-ma`
  - `ndx100-breadth-washout`
  - `overnight-drift`
  - `intraday-drift`

## Output artifacts

- `reports/autoresearch-ledger.jsonl`
- `reports/autoresearch-exa-ideas.json` (when `--seed-web` is set)
- Prefer reading these after each run before accepting candidate upgrades.

## Required behavior for the loop

- Candidate discovery uses arXiv-focused Exa search when `--seed-web` is provided.
- Build a net-new candidate pool from both seeded variants and deterministic grids.
- Run walk-forward train/test windows.
- Score train/test and rank using the existing formulas in the report.
- Keep only candidates that pass JSON parsing and metric gates.

## Environment

- For seed fetching, `EXA_API_KEY` must be loaded in environment.
- For local runs, keep `EXA_API_KEY` in `.env`.
- Start from `.env.example`:
  - `cp .env.example .env`
  - Fill `EXA_API_KEY`.
- One-time sync from shell:
  - `source ~/.zshrc >/dev/null 2>&1 && printf 'EXA_API_KEY=%s\n' \"$EXA_API_KEY\" > .env`

## Notes

- Do not reintroduce Python scripts for the autoresearch pipeline once the Rust binary is present.
- If Python is required for manual data exploration, keep it outside the automated loop path.
