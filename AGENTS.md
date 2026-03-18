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

### Iterative refinement (default)

Looping is the default (`--max-rounds 10`). After round 0, the loop refines top winners by perturbing parameters within discrete grids and swapping assets. Stops on convergence (`--patience 3`, `--min-improvement 0.02`) or frontier exhaustion.

| Flag | Default | Description |
|------|---------|-------------|
| `--max-rounds` | 10 | Maximum refinement rounds |
| `--patience` | 3 | Stale rounds before stopping |
| `--min-improvement` | 0.02 | Minimum relative improvement to reset patience |
| `--refine-top` | 5 | Winners to refine per round |
| `--refine-variants` | 30 | Max variants per winner |
| `--no-loop` | false | Disable refinement (single-pass legacy) |

### Asset universe expansion

Controls which assets are tested during refinement rounds. Round 0 always uses core assets (5) for speed.

| Flag | Default | Description |
|------|---------|-------------|
| `--asset-universe` | `broad` | `core` (5), `broad` (SP500+NDX100 ~550), `full` (all viable warehouse), or preset name |
| `--refine-asset-swaps` | 10 | Max asset swap variants per winner per round |
| `--min-asset-rows` | auto | Min parquet rows for viability (default: train+test sessions) |

### Quality gates

Filter final report to only "investable" strategies. Applied after all rounds complete.

| Flag | Default | Description |
|------|---------|-------------|
| `--min-sharpe` | none | Minimum test-window Sharpe ratio |
| `--max-drawdown` | none | Maximum test-window drawdown (absolute %, e.g. `20` = reject worse than -20%) |

When gates are active, the report shows pass/fail counts. If none pass, unfiltered results shown for reference.

### Convergence summary

When the refinement loop terminates (patience, frontier exhaustion, or max rounds), a diagnostic summary prints:
- Stop reason
- Total evaluated/passed counts and exhausted refinement centers
- Rule and asset distribution among passing candidates
- Score trajectory from first to last round

### Evaluation cache

`reports/autoresearch-eval-cache.jsonl` persists results across runs keyed by parameter signature + date windows. Deterministic grid and repeated seeded candidates are served instantly. Use `--no-cache` to force re-evaluation (e.g., after strategy code changes).

### Audit trail and registry

- Persisted top-10 winners must have train/test audit artifacts under `reports/autoresearch-audits/` with exact evaluated periods, trade ledgers, and equity traces.
- Promoted winners must also be upserted into `reports/autoresearch-strategy-registry.json`, keyed by stable parameter signature, so future implementation work can start from audited candidates rather than only JSONL history.

## Data and candidate constraints

- Backtests use local warehouse parquet data only.
- Loop candidates must execute as `paper-research` only.
- The loop should run with Exa/arXiv candidate seeding via `--seed-web` for new strategy discovery.
- Candidate pool should target at least 100 research candidates by default.

## Output artifacts

- `reports/autoresearch-ledger.jsonl` — append-only log of top-ranked results per run
- `reports/autoresearch-exa-ideas.json` — raw Exa/arXiv seeds (when `--seed-web` is set)
- `reports/autoresearch-top10-interactive-report.html` — branded interactive report
- `reports/autoresearch-audits/` — auditable train/test proof JSONs for persisted top-10 candidates
- `reports/autoresearch-strategy-registry.json` — promoted strategy registry for future real-world implementation work
- `reports/autoresearch-eval-cache.jsonl` — persistent evaluation cache (cross-run)
- Prefer reading these after each production loop before promoting candidates.

## Required behavior for the loop

- Candidate discovery uses arXiv-focused Exa search when `--seed-web` is provided.
- Build net-new candidates from web-seeded hypotheses + deterministic paper-research mutations.
- Run walk-forward train/test windows.
- Score train/test and rank using the loop scoring formula.
- Keep only candidates that pass JSON parsing and metric gates.

## Brand & Visual Design (Mandatory — All Artifacts)

**Every visual artifact produced by agents in this project — HTML reports, dashboards, websites, landing pages, interactive tools, data visualizations, charts, or any other browser-rendered output — MUST follow the doob brand design system.** No exceptions. This applies to all code paths, not just autoresearch reports.

### What to use

- **Template**: `branding/report-template.html` — use this as the base for any autoresearch HTML report. Inject the `rows` JSON array into the `/* PASTE_ROWS_HERE */` slot.
- **Tokens**: `branding/tokens.css` — inline the `:root` block into the report's `<style>`. Reports must be self-contained single-file HTML.
- **Reference**: `branding/brand-guidelines.html` — open in browser to see the full visual system.

### Hard constraints (apply to ALL visual output, not just reports)

1. **Never use the old dark-blue theme** (`#071023`, `#0f1f3a`, `#102548`). It is deprecated. All visual artifacts use the light theme with teal accents.
2. **Always use CSS variables** from `branding/tokens.css` — never hardcode colors. Key tokens:
   - `--doob-teal` (#3e5b63) — hero header, footer, primary panels
   - `--doob-lime` (#c6e758) — accent, positive signals
   - `--doob-sky` (#5fc4e3) — info, links
   - `--doob-slate` (#4a5760) — muted text
   - `--doob-sage` (#c7cdc8) — borders, neutral bg
   - `--doob-positive-text` (#3a5200) — gains; `--doob-negative-text` (#7a1a1a) — losses
3. **Fonts**: display = `var(--doob-font-display)` (Helvetica Now Display + system fallbacks); data = `var(--doob-font-mono)` (DM Mono from Google Fonts).
4. **All numerical data** (CAGR, Sharpe, drawdown, equity, VaR) must use `font-family: var(--doob-font-mono)`.
5. **Section labels**: monospace, uppercase, `letter-spacing: 0.08em`, `font-size: 11px`.
6. **Layout pattern**: teal hero header (rounded bottom), KPI summary bar, filterable data table, expandable row details, teal footer (rounded top).
7. **Pills/badges**: use `.pill-teal`, `.pill-sky`, `.pill-lime` classes for asset tags, rule names, and status indicators.
8. **Load DM Mono**: `<link href="https://fonts.googleapis.com/css2?family=DM+Mono:wght@300;400;500&display=swap" rel="stylesheet" />`
9. **Scope**: These rules apply to ALL visual artifacts — HTML reports, dashboards, charts, websites, landing pages, tools. Anything rendered in a browser must use the doob design system.

### When generating a report

```
1. Read branding/report-template.html
2. Replace /* PASTE_ROWS_HERE */ with the actual rows JSON array
3. Update title, dates, and KPI values
4. Write to reports/autoresearch-top10-interactive-report.html
```

## Environment

- For seed fetching, `EXA_API_KEY` must be loaded in environment.
- `.env` should include this key; start from `.env.example`:
  - `cp .env.example .env`
  - Fill `EXA_API_KEY`.
- One-time sync from shell:
  - `source ~/.zshrc >/dev/null 2>&1 && printf 'EXA_API_KEY=%s\n' "$EXA_API_KEY" > .env`
- `EXA_API_KEY` is required for seeded production loops.
