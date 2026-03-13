"""Tests for doob.metrics — performance and fees."""

from __future__ import annotations

import numpy as np
import pandas as pd
import pytest

from doob.metrics.fees import ibkr_roundtrip_cost
from doob.metrics.performance import (
    annual_returns_table,
    cagr,
    max_drawdown,
    sharpe,
    var_95,
)


# ---------------------------------------------------------------------------
# Performance metrics
# ---------------------------------------------------------------------------
class TestCAGR:
    def test_known_cagr(self):
        equity = np.array([100, 200])
        result = cagr(equity, 10.0)
        assert result == pytest.approx(2 ** (1 / 10) - 1, rel=1e-6)

    def test_zero_years(self):
        assert cagr(np.array([100, 200]), 0) == 0.0

    def test_zero_start(self):
        assert cagr(np.array([0, 200]), 5) == 0.0


class TestSharpe:
    def test_known_sharpe(self):
        rng = np.random.default_rng(42)
        daily_ret = rng.normal(0.001, 0.005, 252)
        s = sharpe(daily_ret, rf=0.0)
        assert s > 0

    def test_zero_vol(self):
        daily_ret = np.full(10, 0.001)
        assert sharpe(daily_ret) == 0.0

    def test_empty(self):
        assert sharpe(np.array([0.01])) == 0.0


class TestMaxDrawdown:
    def test_known_drawdown(self):
        equity = np.array([100, 120, 90, 110, 80])
        assert max_drawdown(equity) == pytest.approx(40 / 120)

    def test_no_drawdown(self):
        equity = np.array([100, 110, 120, 130])
        assert max_drawdown(equity) == pytest.approx(0.0)


class TestVaR95:
    def test_known_var(self):
        rng = np.random.default_rng(42)
        returns = rng.normal(0, 0.01, 10000)
        v = var_95(returns)
        assert v == pytest.approx(-0.01645, abs=0.001)

    def test_with_nans(self):
        returns = np.array([0.01, -0.02, np.nan, 0.005, -0.03, 0.0, -0.01])
        v = var_95(returns)
        assert np.isfinite(v)


class TestAnnualReturnsTable:
    def test_year_filtering(self):
        dates = pd.date_range("2014-01-02", periods=600, freq="B")
        equity_values = np.linspace(100, 200, 600)
        equity = np.concatenate([[100], equity_values])

        tbl = annual_returns_table(equity, dates, start_year=2015)
        assert 2014 not in tbl["year"].values
        assert 2015 in tbl["year"].values

    def test_return_calculation(self):
        dates_2020 = pd.bdate_range("2020-01-02", "2020-12-31")
        dates_2021 = pd.bdate_range("2021-01-04", "2021-12-31")
        dates = dates_2020.append(dates_2021)
        eq_vals = np.concatenate(
            [np.full(len(dates_2020), 100.0), np.full(len(dates_2021), 120.0)]
        )
        equity = np.concatenate([[100], eq_vals])

        tbl = annual_returns_table(equity, pd.Series(dates), start_year=2020)
        yr_2020 = tbl[tbl["year"] == 2020]["return"].iloc[0]
        assert yr_2020 == pytest.approx(0.0, abs=0.001)
        yr_2021 = tbl[tbl["year"] == 2021]["return"].iloc[0]
        assert yr_2021 == pytest.approx(0.0, abs=0.001)


# ---------------------------------------------------------------------------
# Fee model
# ---------------------------------------------------------------------------
class TestIBKRCost:
    def test_normal_cost(self):
        cost = ibkr_roundtrip_cost(100_000, 500)
        shares = 200
        per_side = shares * 0.0065
        assert cost == pytest.approx(per_side * 2)

    def test_min_order(self):
        cost = ibkr_roundtrip_cost(100, 100)
        assert cost == pytest.approx(0.35 * 2)

    def test_max_cap(self):
        cost = ibkr_roundtrip_cost(10_000, 0.10)
        shares = 100_000
        trade_value = shares * 0.10
        max_per_side = 0.01 * trade_value
        raw_per_side = shares * 0.0065
        assert cost == pytest.approx(max_per_side * 2)
        assert raw_per_side > max_per_side

    def test_zero_shares(self):
        assert ibkr_roundtrip_cost(0, 100) == 0.0
        assert ibkr_roundtrip_cost(50, 100) == 0.0
