"""Intraday Drift Backtesting Engine.

Buy SPY at the open, sell at the close on the same day.
This captures the intraday return component, complementary to the overnight drift.

Note: adj_close == close in this warehouse (IB TRADES data, split-adjusted
but not dividend-adjusted). For intraday returns ln(Close_t / Open_t)
this is fine — both prices are same-day so no dividend adjustment needed.
"""

from __future__ import annotations

import argparse
import logging
from pathlib import Path

import matplotlib
import numpy as np
import pandas as pd

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

from doob.config import output_dir
from doob.data.readers import load_ticker_ohlcv
from doob.metrics.fees import ibkr_roundtrip_cost
from doob.metrics.performance import (
    annual_returns_table,
    cagr,
    max_drawdown,
    sharpe,
    var_95,
)

log = logging.getLogger(__name__)

DEFAULT_CAPITAL = 1_000_000.0
OUTPUT_DIR = output_dir()


# ---------------------------------------------------------------------------
# Signal computation
# ---------------------------------------------------------------------------
def compute_intraday_returns(df: pd.DataFrame) -> pd.Series:
    """Compute intraday log returns: ln(Close_t / Open_t)."""
    return np.log(df["close"] / df["open"])


# ---------------------------------------------------------------------------
# Strategy simulation
# ---------------------------------------------------------------------------
def simulate_strategy(
    opens: np.ndarray,
    closes: np.ndarray,
    mask: np.ndarray,
    capital: float = DEFAULT_CAPITAL,
    fee_fn=ibkr_roundtrip_cost,
    short: bool = False,
) -> np.ndarray:
    """Simulate intraday strategy with equity-tracking loop.

    Long: buy at open, sell at close same day.
    Short: sell at open, cover at close same day.

    Returns:
        equity curve array (length = len(opens) + 1, starting at capital)
    """
    n = len(opens)
    equity = np.empty(n + 1)
    equity[0] = capital
    current = capital
    direction = -1.0 if short else 1.0

    for i in range(n):
        if mask[i]:
            shares = int(current / opens[i])
            if shares > 0:
                cost = fee_fn(current, opens[i])
                pnl = direction * shares * (closes[i] - opens[i])
                current = current + pnl - cost
        equity[i + 1] = current

    return equity


# ---------------------------------------------------------------------------
# Plotting
# ---------------------------------------------------------------------------
def plot_dashboard(  # pragma: no cover
    results: dict,
    dates: pd.Series,
    ticker: str = "SPY",
    output_dir: Path = OUTPUT_DIR,
) -> None:
    """Render equity curves and rolling volatility charts."""
    output_dir.mkdir(parents=True, exist_ok=True)
    slug = ticker.lower()

    fig, ax = plt.subplots(figsize=(14, 7))
    for name, data in results.items():
        ax.plot(dates, data["equity"][1:], label=name, linewidth=0.8)
    ax.set_title(f"Buy the Open, Sell the Close ({ticker}) — Equity Curves")
    ax.set_ylabel("Portfolio Value ($)")
    ax.set_xlabel("Date")
    ax.legend()
    ax.grid(True, alpha=0.3)
    ax.set_yscale("log")
    fig.tight_layout()
    fig.savefig(output_dir / f"intraday_drift_{slug}_equity.png", dpi=150)
    plt.close(fig)

    fig, ax = plt.subplots(figsize=(14, 5))
    for name, data in results.items():
        daily_ret = np.diff(data["equity"]) / data["equity"][:-1]
        rolling_vol = pd.Series(daily_ret).rolling(63).std() * np.sqrt(252)
        ax.plot(dates, rolling_vol.values, label=name, linewidth=0.8)
    ax.set_title(f"Buy the Open, Sell the Close ({ticker}) — Rolling 63-day Annualized Volatility")
    ax.set_ylabel("Volatility")
    ax.set_xlabel("Date")
    ax.legend()
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(output_dir / f"intraday_drift_{slug}_volatility.png", dpi=150)
    plt.close(fig)

    log.info("Charts saved to %s", output_dir)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main() -> None:  # pragma: no cover
    parser = argparse.ArgumentParser(description="Buy the Open, Sell the Close Backtest")
    parser.add_argument("--start-date", type=str, default=None, help="Start date (YYYY-MM-DD)")
    parser.add_argument("--end-date", type=str, default=None, help="End date (YYYY-MM-DD)")
    parser.add_argument("--capital", type=float, default=DEFAULT_CAPITAL, help="Starting capital")
    parser.add_argument("--no-plots", action="store_true", help="Skip chart generation")
    parser.add_argument("--ticker", type=str, default="SPY", help="Ticker symbol (default: SPY)")
    parser.add_argument("--short", action="store_true", help="Short at open, cover at close")
    parser.add_argument("--start-year-table", type=int, default=2015, help="Annual table start year")
    args = parser.parse_args()

    ticker = args.ticker.upper()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

    print(f"Loading {ticker} from bronze parquet...")
    spy = load_ticker_ohlcv(ticker)
    print(f"  {ticker}: {len(spy):,} bars, {spy['trade_date'].min().date()} to {spy['trade_date'].max().date()}")

    if args.start_date:
        spy = spy[spy["trade_date"] >= pd.Timestamp(args.start_date)]
    if args.end_date:
        spy = spy[spy["trade_date"] <= pd.Timestamp(args.end_date)]
    spy = spy.reset_index(drop=True)

    dates = spy["trade_date"]
    n = len(spy)
    opens = spy["open"].values
    closes = spy["close"].values

    strategies = {}

    bh_equity = np.empty(n + 1)
    bh_equity[0] = args.capital
    shares_bh = int(args.capital / closes[0])
    cost_bh = ibkr_roundtrip_cost(args.capital, closes[0])
    for i in range(n):
        bh_equity[i + 1] = shares_bh * closes[i] + (args.capital - shares_bh * closes[0]) - cost_bh
    strategies["Buy & Hold"] = {"equity": bh_equity}

    mask_all = np.ones(n, dtype=bool)
    eq_intra = simulate_strategy(opens, closes, mask_all, args.capital, short=args.short)
    label = "Short Open→Cover Close" if args.short else "Intraday (Open→Close)"
    strategies[label] = {"equity": eq_intra}

    years = (dates.iloc[-1] - dates.iloc[0]).days / 365.25

    print("\n" + "=" * 80)
    mode = "SHORT OPEN, COVER CLOSE" if args.short else "BUY THE OPEN, SELL THE CLOSE"
    print(f"{mode} — {ticker} BACKTEST RESULTS")
    print(f"Period: {dates.iloc[0].date()} to {dates.iloc[-1].date()} ({years:.1f} years)")
    print(f"Capital: ${args.capital:,.0f} | Fee model: IBKR Tiered")
    print("Note: adj_close == close (IB split-adj only); B&H CAGR understates by ~1.3%/yr")
    print("=" * 80)

    header = f"{'Strategy':<25} {'Final ($)':>14} {'CAGR':>8} {'Sharpe':>8} {'MaxDD':>8} {'VaR95':>8}"
    print(header)
    print("-" * len(header))

    for name, data in strategies.items():
        eq = data["equity"]
        daily_returns = np.diff(eq) / eq[:-1]
        c = cagr(eq, years)
        s = sharpe(daily_returns)
        md = max_drawdown(eq)
        v = var_95(daily_returns)
        print(f"{name:<25} {eq[-1]:>14,.0f} {c:>7.1%} {s:>8.2f} {md:>7.1%} {v:>8.4f}")

    print(f"\nAnnual Returns (from {args.start_year_table}):")
    ann_header = f"{'Year':<6}"
    for name in strategies:
        ann_header += f" {name:>25}"
    print(ann_header)
    print("-" * len(ann_header))

    annual_tables = {}
    for name, data in strategies.items():
        annual_tables[name] = annual_returns_table(data["equity"], dates, args.start_year_table)

    all_years: set[int] = set()
    for tbl in annual_tables.values():
        all_years.update(tbl["year"].tolist())

    for year in sorted(all_years):
        row = f"{year:<6}"
        for name in strategies:
            tbl = annual_tables[name]
            yr_row = tbl[tbl["year"] == year]
            if len(yr_row) > 0:
                row += f" {yr_row['return'].iloc[0]:>24.1%}"
            else:
                row += f" {'N/A':>25}"
        print(row)

    if not args.no_plots:
        print(f"\nGenerating charts to {OUTPUT_DIR}/...")
        plot_dashboard(results=strategies, dates=dates, ticker=ticker)
        print("Done.")


if __name__ == "__main__":
    main()
