# doob AGENTS (Codex Working Instructions)

## Scope

This file is the codex-specific execution guide for this repository.
It complements `CLAUDE.md` with mandatory constraints for automated work.

## Core rule

- Autoresearch loops MUST be implemented and executed in Rust.
- Do not use Python for looping or orchestration of candidate generation, backtest runs, scoring, or ranking.
- This repository’s automated discovery workflow is paper-research-first: use arXiv/Exa seeds, then execute as `paper-research`.

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
  - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose`
  - `cargo run --release --bin autoresearch_loop -- --seed-web --verbose`
  - Optional dates/sessions override:
    - `--train-start 2020-01-01 --train-end 2024-12-31 --test-start 2025-01-01 --test-end 2026-03-11 --train-sessions 1008 --test-sessions 252`
  - Optional binary override:
    - `--doob-bin target/release/doob`

## Data and candidate constraints

- Backtests use local warehouse parquet data only.
- Loop candidates must execute as `paper-research` only.
- The loop should run with Exa/arXiv candidate seeding via `--seed-web` for new strategy discovery.
- Candidate pool should target at least 100 research candidates by default.

## Output artifacts

- `reports/autoresearch-ledger.jsonl`
- `reports/autoresearch-exa-ideas.json` (when `--seed-web` is set)
- `reports/autoresearch-top10-interactive-report.html`
- Prefer reading these after each production loop before promoting candidates.

## Required behavior for the loop

- Candidate discovery uses arXiv-focused Exa search when `--seed-web` is provided.
- Build net-new candidates from web-seeded hypotheses + deterministic paper-research mutations.
- Run walk-forward train/test windows.
- Score train/test and rank using the loop scoring formula.
- Keep only candidates that pass JSON parsing and metric gates.

## Environment

- For seed fetching, `EXA_API_KEY` must be loaded in environment.
- `.env` should include this key; start from `.env.example`:
  - `cp .env.example .env`
  - Fill `EXA_API_KEY`.
- One-time sync from shell:
  - `source ~/.zshrc >/dev/null 2>&1 && printf 'EXA_API_KEY=%s\n' "$EXA_API_KEY" > .env`
- `EXA_API_KEY` is required for seeded production loops.
