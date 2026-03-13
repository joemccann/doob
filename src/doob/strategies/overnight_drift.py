"""Overnight Drift Backtesting Engine.

Vectorized backtest for the "Overnight Drift" anomaly:
  Buy SPY at close, sell at next open.
Optional VIX regime filter: only take overnight trades when VIX < 200-day MA.

SPY data comes from the bronze parquet lake via DuckDB.
VIX data comes from CBOE's public CSV endpoint (cached locally).

Note: adj_close == close in this warehouse (IB TRADES data, split-adjusted
but not dividend-adjusted). For overnight returns ln(Open_{t+1}/Close_t)
this is fine — the signal is close-to-open price movement. Buy-and-hold CAGR
will understate true total return by ~1.3%/yr due to missing dividends.
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
from statsmodels.tsa.stattools import adfuller  # noqa: E402

from doob.config import output_dir
from doob.data.readers import load_ticker_ohlcv, load_vix_from_cboe
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
def compute_overnight_returns(df: pd.DataFrame) -> pd.Series:
    """Compute overnight log returns: ln(Open_{t+1} / Close_t).

    Last row will be NaN (no next open).
    """
    next_open = df["open"].shift(-1)
    return np.log(next_open / df["close"])


def compute_vix_filter(vix_df: pd.DataFrame, lookback: int = 200) -> pd.DataFrame:
    """Add VIX MA and filter mask to VIX DataFrame.

    Returns DataFrame with trade_date, vix_close, vix_ma, vix_filter columns.
    vix_filter is True when VIX close < VIX MA (low-vol regime).
    """
    result = pd.DataFrame()
    result["trade_date"] = vix_df["trade_date"]
    result["vix_close"] = vix_df["close"].values
    result["vix_ma"] = vix_df["close"].rolling(window=lookback, min_periods=lookback).mean().values
    result["vix_filter"] = result["vix_close"] < result["vix_ma"]
    return result


# ---------------------------------------------------------------------------
# Strategy simulation
# ---------------------------------------------------------------------------
def simulate_strategy(
    returns: np.ndarray,
    closes: np.ndarray,
    opens_next: np.ndarray,
    mask: np.ndarray,
    capital: float = DEFAULT_CAPITAL,
    fee_fn=ibkr_roundtrip_cost,
) -> np.ndarray:
    """Simulate overnight strategy with equity-tracking loop.

    Args:
        returns: overnight log returns array
        closes: close prices (buy price)
        opens_next: next-day open prices (sell price)
        mask: boolean array — True means take the trade
        capital: starting capital
        fee_fn: callable(equity, price) -> dollar cost

    Returns:
        equity curve array (length = len(returns) + 1, starting at capital)
    """
    n = len(returns)
    equity = np.empty(n + 1)
    equity[0] = capital
    current = capital

    for i in range(n):
        if mask[i] and np.isfinite(returns[i]):
            shares = int(current / closes[i])
            if shares > 0:
                cost = fee_fn(current, closes[i])
                pnl = shares * (opens_next[i] - closes[i])
                current = current + pnl - cost
            # else: no change
        equity[i + 1] = current

    return equity


# ---------------------------------------------------------------------------
# Analytics
# ---------------------------------------------------------------------------
def adf_test(returns: np.ndarray) -> dict:
    """Augmented Dickey-Fuller test on returns series."""
    clean = returns[np.isfinite(returns)]
    stat, pvalue, *_ = adfuller(clean, maxlag=10, autolag="AIC")
    return {"adf_statistic": stat, "p_value": pvalue}


# ---------------------------------------------------------------------------
# Plotting
# ---------------------------------------------------------------------------
def plot_dashboard(  # pragma: no cover
    results: dict,
    dates: pd.Series,
    output_dir: Path = OUTPUT_DIR,
) -> None:
    """Render equity curves and rolling volatility charts."""
    output_dir.mkdir(parents=True, exist_ok=True)

    fig, ax = plt.subplots(figsize=(14, 7))
    for name, data in results.items():
        ax.plot(dates, data["equity"][1:], label=name, linewidth=0.8)
    ax.set_title("Overnight Drift — Equity Curves")
    ax.set_ylabel("Portfolio Value ($)")
    ax.set_xlabel("Date")
    ax.legend()
    ax.grid(True, alpha=0.3)
    ax.set_yscale("log")
    fig.tight_layout()
    fig.savefig(output_dir / "overnight_drift_equity.png", dpi=150)
    plt.close(fig)

    fig, ax = plt.subplots(figsize=(14, 5))
    for name, data in results.items():
        daily_ret = np.diff(data["equity"]) / data["equity"][:-1]
        rolling_vol = pd.Series(daily_ret).rolling(63).std() * np.sqrt(252)
        ax.plot(dates, rolling_vol.values, label=name, linewidth=0.8)
    ax.set_title("Overnight Drift — Rolling 63-day Annualized Volatility")
    ax.set_ylabel("Volatility")
    ax.set_xlabel("Date")
    ax.legend()
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(output_dir / "overnight_drift_volatility.png", dpi=150)
    plt.close(fig)

    log.info("Charts saved to %s", output_dir)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main() -> None:  # pragma: no cover
    parser = argparse.ArgumentParser(description="Overnight Drift Backtest")
    parser.add_argument("--start-date", type=str, default=None, help="Start date (YYYY-MM-DD)")
    parser.add_argument("--end-date", type=str, default=None, help="End date (YYYY-MM-DD)")
    parser.add_argument("--capital", type=float, default=DEFAULT_CAPITAL, help="Starting capital")
    parser.add_argument("--no-vix-filter", action="store_true", help="Skip VIX-filtered strategy")
    parser.add_argument("--no-plots", action="store_true", help="Skip chart generation")
    parser.add_argument("--start-year-table", type=int, default=2015, help="Annual table start year")
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

    print("Loading SPY from bronze parquet...")
    spy = load_ticker_ohlcv("SPY")
    print(f"  SPY: {len(spy):,} bars, {spy['trade_date'].min().date()} to {spy['trade_date'].max().date()}")

    include_vix = not args.no_vix_filter
    if include_vix:
        print("Loading VIX from CBOE...")
        vix_raw = load_vix_from_cboe()
        vix = compute_vix_filter(vix_raw)
        print(f"  VIX: {len(vix_raw):,} bars, {vix_raw['trade_date'].min().date()} to {vix_raw['trade_date'].max().date()}")

    if args.start_date:
        spy = spy[spy["trade_date"] >= pd.Timestamp(args.start_date)]
    if args.end_date:
        spy = spy[spy["trade_date"] <= pd.Timestamp(args.end_date)]
    spy = spy.reset_index(drop=True)

    overnight = compute_overnight_returns(spy)
    spy["overnight_return"] = overnight.values
    spy["open_next"] = spy["open"].shift(-1)

    if include_vix:
        spy = spy.merge(vix[["trade_date", "vix_close", "vix_ma", "vix_filter"]], on="trade_date", how="left")
        spy["vix_filter"] = spy["vix_filter"].fillna(False)
    else:
        spy["vix_filter"] = False

    spy = spy.iloc[:-1].reset_index(drop=True)
    dates = spy["trade_date"]

    n = len(spy)
    ret = spy["overnight_return"].values
    closes = spy["close"].values
    opens_next = spy["open_next"].values

    strategies = {}

    bh_equity = np.empty(n + 1)
    bh_equity[0] = args.capital
    shares_bh = int(args.capital / closes[0])
    cost_bh = ibkr_roundtrip_cost(args.capital, closes[0])
    for i in range(n):
        bh_equity[i + 1] = shares_bh * closes[i] + (args.capital - shares_bh * closes[0]) - cost_bh
    strategies["Buy & Hold"] = {"equity": bh_equity}

    mask_all = np.ones(n, dtype=bool)
    eq_all = simulate_strategy(ret, closes, opens_next, mask_all, args.capital)
    strategies["Overnight (All)"] = {"equity": eq_all}

    if include_vix:
        mask_vix = spy["vix_filter"].values.astype(bool)
        eq_vix = simulate_strategy(ret, closes, opens_next, mask_vix, args.capital)
        strategies["Overnight (VIX Filter)"] = {"equity": eq_vix}

    years = (dates.iloc[-1] - dates.iloc[0]).days / 365.25

    print("\n" + "=" * 80)
    print("OVERNIGHT DRIFT BACKTEST RESULTS")
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

        if "Overnight" in name:
            adf = adf_test(daily_returns)
            print(f"  {'ADF stat':>23}: {adf['adf_statistic']:.4f}  p-value: {adf['p_value']:.6f}")

    print(f"\nAnnual Returns (from {args.start_year_table}):")
    ann_header = f"{'Year':<6}"
    for name in strategies:
        ann_header += f" {name:>20}"
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
                row += f" {yr_row['return'].iloc[0]:>19.1%}"
            else:
                row += f" {'N/A':>20}"
        print(row)

    if not args.no_plots:
        print(f"\nGenerating charts to {OUTPUT_DIR}/...")
        plot_dashboard(results=strategies, dates=dates)
        print("Done.")

    if include_vix and "Overnight (VIX Filter)" in strategies:
        eq_all_final = strategies["Overnight (All)"]["equity"]
        eq_vix_final = strategies["Overnight (VIX Filter)"]["equity"]
        dr_all = np.diff(eq_all_final) / eq_all_final[:-1]
        dr_vix = np.diff(eq_vix_final) / eq_vix_final[:-1]
        s_all = sharpe(dr_all)
        s_vix = sharpe(dr_vix)
        vix_days = int(spy["vix_filter"].sum())
        total_days = len(spy)
        print(f"\nVIX Filter traded {vix_days:,}/{total_days:,} days ({vix_days/total_days:.0%})")
        if s_vix > s_all:
            print(f"VIX filter improved Sharpe: {s_all:.2f} -> {s_vix:.2f} by avoiding high-vol gap risk")
        else:
            print(f"VIX filter Sharpe: {s_vix:.2f} vs unfiltered: {s_all:.2f}")


if __name__ == "__main__":
    main()
