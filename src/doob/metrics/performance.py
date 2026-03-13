"""Performance metrics — extracted from the overnight/intraday drift strategies."""

from __future__ import annotations

import numpy as np
import pandas as pd

RISK_FREE_RATE = 0.04  # 4% annual


def cagr(equity: np.ndarray, years: float) -> float:
    """Compound annual growth rate."""
    if years <= 0 or equity[0] <= 0:
        return 0.0
    return (equity[-1] / equity[0]) ** (1.0 / years) - 1.0


def sharpe(returns: np.ndarray, rf: float = RISK_FREE_RATE) -> float:
    """Annualized Sharpe ratio from daily returns."""
    daily_rf = rf / 252
    excess = returns - daily_rf
    if len(excess) < 2 or np.std(excess) == 0:
        return 0.0
    return np.mean(excess) / np.std(excess, ddof=1) * np.sqrt(252)


def max_drawdown(equity: np.ndarray) -> float:
    """Maximum drawdown as a positive fraction."""
    peak = np.maximum.accumulate(equity)
    dd = (peak - equity) / peak
    return float(np.max(dd))


def var_95(returns: np.ndarray) -> float:
    """95% Value at Risk (historical, daily)."""
    return float(np.percentile(returns[np.isfinite(returns)], 5))


def annual_returns_table(
    equity: np.ndarray, dates: pd.Series, start_year: int = 2015
) -> pd.DataFrame:
    """Per-year returns from equity curve."""
    df = pd.DataFrame({"date": dates, "equity": equity[1:]})  # skip initial capital
    df["year"] = df["date"].dt.year
    df = df[df["year"] >= start_year]

    rows = []
    for year, grp in df.groupby("year"):
        first = grp["equity"].iloc[0]
        last = grp["equity"].iloc[-1]
        ret = (last / first) - 1.0
        rows.append({"year": int(year), "return": ret})

    return pd.DataFrame(rows)
