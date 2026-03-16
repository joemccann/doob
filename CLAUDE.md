# doob — Quantitative Strategy Research & Backtesting

Rust binary for quantitative strategy research. Reads from the shared `~/market-warehouse/` data lake (bronze parquet). **All price data comes from local parquet files — no Yahoo Finance or external API calls for price data.** Universe membership is resolved from local preset JSON files (e.g. `presets/ndx100.json`). The only external HTTP call is the optional CBOE VIX CSV download (cached for 24h).

## Autoresearch Loop (Mandatory Rust-Only Path)

- Use `cargo build --release` (or `cargo build` for debug) to build the Rust binaries.
- Run autoresearch with the Rust binary, not Python:
  - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose`
  - verbose trace:
    - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose`
  - default output now includes strategy category, candidate assets, horizon, and rationale summary in the results table.
  - **production mode is paper-research only**: arXiv/Exa-hypothesis-driven loops are executed as `paper-research` strategies.
  - `cargo run --release --bin autoresearch_loop -- --doob-bin target/release/doob --seed-web --train-start 2020-01-01 --train-end 2024-12-31 --test-start 2025-01-01 --test-end 2026-03-11 --train-sessions 1008 --test-sessions 252`
- Results are appended to `reports/autoresearch-ledger.jsonl` and `reports/autoresearch-exa-ideas.json`.
- Interactive report output: `reports/autoresearch-top10-interactive-report.html`.
- Exa seeding uses `EXA_API_KEY` from environment; keep python script usage out of the autoresearch path.
- Use `.env.example` as a starter file:
  - `cp .env.example .env`
  - `EXA_API_KEY=your_exa_api_key_here`
- Then run:
  - `printf 'EXA_API_KEY=%s\\n' \"$EXA_API_KEY\" > .env`
  - `cargo run --release --bin autoresearch_loop -- --seed-web`

## Crate Layout

```
src/
├── main.rs                          # Binary entrypoint (includes execution timer)
├── lib.rs                           # Library root (re-exports all modules)
├── cli.rs                           # Unified CLI: doob run <strategy> | list-strategies | list-presets
├── config.rs                        # Centralized config: warehouse path, output root, presets dir
├── data/
│   ├── mod.rs
│   ├── paths.rs                     # Parquet path resolution helpers
│   ├── discovery.rs                 # Symbol discovery from bronze layer
│   ├── readers.rs                   # Parquet (polars) data loaders, load_price_panel(), CBOE VIX cache
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
    ├── breadth_washout.rs           # Generic breadth signal across any universe (default: 5-day SMA)
    ├── breadth_ma.rs                # Single MA breadth (default: 50-day); % below/above N-day MA
    ├── breadth_dual_ma.rs           # Dual MA breadth; close < short MA AND close > long MA
    ├── ndx100_sma_breadth.rs        # NDX-100 SMA breadth analysis + forward returns
    └── ndx100_breadth_washout.rs    # Thin wrapper
```

## Data Architecture

**All price data is read from local warehouse parquet files.** No Yahoo Finance, no external price APIs.

- Price data: `~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet`
- Universe membership: `presets/<universe>.json` (e.g. `presets/ndx100.json`, `presets/sp500.json`)
- VIX data: CBOE CSV, cached locally for 24h (only external HTTP call)

Expected parquet columns: `trade_date`, `open`, `high`, `low`, `close`, `volume`.

**Important**: `adj_close == close` in this warehouse (IB TRADES data is split-adjusted but not dividend-adjusted). Buy-and-hold CAGR will understate true total return by ~1.3%/yr due to missing dividends.

### Data flow

1. Universe tickers loaded from `presets/<name>.json` (static membership, no API)
2. Price data for each ticker loaded from warehouse parquet via `load_price_panel()`
3. Forward-return asset prices (QQQ, TQQQ, SPY, etc.) also from warehouse parquet
4. Trading calendar derived from lead forward asset's parquet dates
5. All computation is local — a 10-year, 101-ticker backtest runs in ~0.3 seconds

## Config Precedence

`DOOB_WAREHOUSE_PATH` env var → `.env` file → `~/market-warehouse` (default)

## Strategy Catalog

```bash
doob run overnight-drift --no-vix-filter --no-plots
doob run intraday-drift --ticker SPY --short
doob run paper-research --output json --asset TQQQ --rule rsi_reversion --fast-window 16 --slow-window 18 --rsi-window 16 --rsi-oversold 28 --rsi-overbought 76 --vol-window 20 --vol-cap 0.40
doob run breadth-washout --universe ndx100 --signal-mode oversold
doob run breadth-washout --universe ndx100 --lookback 50 --signal-mode oversold --threshold 80
doob run ndx100-sma-breadth --end-date 2026-03-11
doob run breadth-ma --universe ndx100 --short-period 50 --signal-mode oversold --threshold 80
doob run breadth-dual-ma --universe ndx100 --short-period 50 --long-period 200 --threshold 20
doob list-strategies
doob list-presets
```

### breadth-washout

Generic breadth signal strategy. Computes % of a universe above/below an N-day SMA
(`--lookback`, default 5). Triggers oversold/overbought at a threshold. Computes
forward returns on configurable assets (SPY, SPXL, QQQ, TQQQ, etc.) with full
risk metrics (Sharpe, Sortino, max drawdown, VaR, CVaR, profit factor).

Key flags: `--lookback 5` (SMA period), `--threshold 65` (trigger level),
`--signal-mode oversold|overbought`, `--assets QQQ TQQQ`, `--sessions 5831`.

Universe membership is loaded from preset JSON files. All price data comes from
local warehouse parquet — no network calls.

Output: `_summary.csv`, `_triggers.csv`, `_membership_changes.csv`, `.json` meta,
`_viz.json` (dashboard consumption). Max drawdown in output is the compounded equity
curve drawdown (not worst single trade).

### breadth-ma

Breadth strategy using a single configurable MA period (default 50-day). Computes
% of universe below/above the N-day MA and triggers signals at a threshold. Same
forward-return and risk-metric pipeline as breadth-washout. Supports the same
universe modes (ndx100, sp500, r2k, all-stocks, preset, tickers).

### breadth-dual-ma

Dual moving-average breadth strategy. For each stock, checks TWO conditions:
`close < short-period MA AND close > long-period MA`. This identifies stocks in
a short-term pullback while still in a long-term uptrend. Computes the % of the
universe meeting both conditions simultaneously, and triggers signals when that
% crosses a threshold.

## Universe Modes

All breadth strategies resolve universe membership locally:

| Universe | Source | Description |
|----------|--------|-------------|
| `ndx100` | `presets/ndx100.json` | NASDAQ-100 constituents (101 tickers) |
| `sp500` | `presets/sp500.json` | S&P 500 constituents |
| `r2k` | `presets/r2k.json` | Russell 2000 constituents |
| `all-stocks` | warehouse bronze dir scan | All symbols in the warehouse |
| `--preset <path>` | custom JSON file | Any custom ticker list |
| `--tickers AAPL,MSFT` | CLI argument | Explicit ticker list |

Preset JSON format: `{ "name": "ndx100", "tickers": ["AAPL", "MSFT", ...] }`

## Output Formats

All strategies support three output formats via the global `--output` flag:

| Format | Flag | Description |
|--------|------|-------------|
| Text | `--output text` | Default. Human-readable tables with progress messages. |
| JSON | `--output json` | Structured JSON object. No progress text. |
| Markdown | `--output md` | Clean markdown with headers and tables. No progress text. |

```bash
doob --output json run overnight-drift --no-vix-filter
doob --output md run breadth-washout --universe ndx100
doob run intraday-drift --ticker SPY --output json
```

The flag is global and can appear before or after the subcommand.

## Install & Update

Build and install to `~/.cargo/bin` (must be in `$PATH`):

```bash
cargo build --release
cp target/release/doob ~/.cargo/bin/doob
```

After any code changes, rebuild and reinstall:

```bash
cargo build --release && cp target/release/doob ~/.cargo/bin/doob
```

## Testing

```bash
# Unit tests (145 tests, < 0.1s, no external dependencies)
cargo test

# CLI integration tests (106 tests, requires ~/market-warehouse)
./tests/cli_integration.sh
```

### Test Rules

1. 146 unit tests covering all modules (mock all I/O, use `tempfile`)
2. 106 CLI integration tests covering every command, flag combination, output format (text/json/md), and error case
3. Tests run with `set -euo pipefail` — any unexpected failure stops the suite
4. Edge cases tested: future dates, 0 sessions, missing tickers, invalid modes, invalid output formats

## Key Dependencies

- `polars` — DataFrame & parquet I/O
- `nalgebra` — Linear algebra (ADF test OLS)
- `clap` — CLI argument parsing (derive), global `--output` flag
- `serde` + `serde_json` — JSON serialization for `--output json`
- `reqwest` — HTTP client (CBOE VIX download only; no price data fetching)
- `chrono` — Date/time operations

## Performance

All price data reads from local parquet. Typical benchmarks on Apple Silicon:

| Workload | Time |
|----------|------|
| 1-year backtest (252 sessions, 101 tickers) | ~0.16s |
| 10-year backtest (2,516 sessions, 101 tickers) | ~0.32s |
| 10-year dual-MA (2,516 sessions, 101 tickers) | ~0.55s |

## Brand & Report Styling (Mandatory)

All generated HTML reports **must** use the doob design system derived from radial.org. This is non-negotiable.

### Design System Location

```
branding/
├── tokens.css              # CSS custom properties (design tokens)
├── brand-guidelines.html   # Visual reference — open in browser to preview
└── report-template.html    # Drop-in template for autoresearch HTML reports
```

### Rules for HTML Report Generation

1. **Use `branding/report-template.html` as the base** for any autoresearch HTML report.
2. **Inline the tokens from `branding/tokens.css`** (the `:root` block) into the report `<style>` tag — reports must be self-contained single-file HTML.
3. **Required design tokens** — always reference these CSS variables, never hardcode colors:
   - `--doob-teal` (#3e5b63) — headers, footer, primary panels
   - `--doob-lime` (#c6e758) — accent, positive signals, CTAs
   - `--doob-sky` (#5fc4e3) — info highlights, links
   - `--doob-slate` (#4a5760) — muted text, labels
   - `--doob-sage` (#c7cdc8) — borders, neutral backgrounds
   - `--doob-positive-text` / `--doob-negative-text` — financial gain/loss coloring
4. **Fonts**: primary = `var(--doob-font-display)` (Helvetica Now Display with system fallback stack); data/mono = `var(--doob-font-mono)` (DM Mono via Google Fonts).
5. **Typography conventions**:
   - Section labels: `font-family: var(--doob-font-mono)`, `text-transform: uppercase`, `letter-spacing: 0.08em`, `font-size: 11px`
   - All numerical data (CAGR, Sharpe, drawdown, equity): `font-family: var(--doob-font-mono)`
   - Positive values: `color: var(--doob-positive-text)`; negative values: `color: var(--doob-negative-text)`
6. **Layout**: teal hero header with `border-radius: 0 0 24px 24px`, KPI summary bar, filterable/sortable data table, teal footer with `border-radius: 24px 24px 0 0`.
7. **No dark-blue themes** — the old `#071023` / `#0f1f3a` dark-blue report style is deprecated. All new reports use the light theme with teal accents.
8. **Components**: use pills (`.pill-teal`, `.pill-sky`, `.pill-lime`) for tags/badges, expandable row details for per-strategy deep-dive, and the KPI card pattern for summary metrics.
9. **Load DM Mono** from Google Fonts: `<link href="https://fonts.googleapis.com/css2?family=DM+Mono:wght@300;400;500&display=swap" rel="stylesheet" />`

### Quick Reference: Color Palette

| Token | Hex | Use |
|-------|-----|-----|
| `--doob-teal` | #3e5b63 | Primary brand, hero bg, footer |
| `--doob-teal-deep` | #2e454c | Darker teal variant |
| `--doob-lime` | #c6e758 | Accent, positive, CTA arrows |
| `--doob-sky` | #5fc4e3 | Info, highlights, links |
| `--doob-slate` | #4a5760 | Muted text, secondary |
| `--doob-sage` | #c7cdc8 | Borders, neutral surface |
| `--doob-bg` | #ffffff | Page background |
| `--doob-bg-alt` | #f5f5f5 | Alt surface, table headers |
| `--doob-text` | #1e1e1e | Primary text |
| `--doob-positive-text` | #3a5200 | Gains, positive CAGR |
| `--doob-negative-text` | #7a1a1a | Losses, drawdown |
| `--doob-warning-text` | #7a4a00 | Warnings, mixed signals |

## How to Add a New Strategy

1. Create `src/strategies/my_strategy.rs` with a `run(args, fmt)` function
2. Define `MyStrategyArgs` using clap derive
3. Load prices with `crate::data::readers::load_price_panel()` — never fetch from external APIs
4. Load universe membership from preset JSON or CLI tickers — never call external membership APIs
5. Add the strategy to `StrategyCommand` enum in `src/cli.rs`
6. Wire it up in `src/main.rs` match arm
7. Write tests in the `#[cfg(test)] mod tests` block
8. Run `cargo test`
