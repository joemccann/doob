"""Tests for the overnight drift backtesting engine.

All synthetic data — no file or network I/O.
"""

from __future__ import annotations


import numpy as np
import pandas as pd
import pytest

from doob.strategies.overnight_drift import (
    adf_test,
    compute_overnight_returns,
    compute_vix_filter,
    simulate_strategy,
)


def _spy_df(opens, closes):
    """Build a minimal SPY-like DataFrame."""
    n = len(opens)
    dates = pd.date_range("2024-01-02", periods=n, freq="B")
    return pd.DataFrame(
        {
            "trade_date": dates,
            "open": opens,
            "high": [max(o, c) + 1 for o, c in zip(opens, closes)],
            "low": [min(o, c) - 1 for o, c in zip(opens, closes)],
            "close": closes,
            "volume": [1_000_000] * n,
        }
    )


class TestAdfTest:
    def test_returns_dict(self):
        rng = np.random.default_rng(42)
        returns = rng.normal(0, 0.01, 500)
        result = adf_test(returns)
        assert "adf_statistic" in result
        assert "p_value" in result
        assert result["p_value"] < 0.05  # random returns should be stationary


class TestComputeOvernightReturns:
    def test_basic_log_returns(self):
        df = _spy_df([100, 102, 104], [101, 103, 105])
        result = compute_overnight_returns(df)
        expected_0 = np.log(102 / 101)
        expected_1 = np.log(104 / 103)
        np.testing.assert_almost_equal(result.iloc[0], expected_0)
        np.testing.assert_almost_equal(result.iloc[1], expected_1)
        assert np.isnan(result.iloc[2])

    def test_single_row(self):
        df = _spy_df([100], [101])
        result = compute_overnight_returns(df)
        assert np.isnan(result.iloc[0])


class TestComputeVixFilter:
    def test_ma_crossover(self):
        n = 210
        closes = [30.0] * 200 + [15.0] * 10
        dates = pd.date_range("2020-01-01", periods=n, freq="B")
        vix_df = pd.DataFrame({"trade_date": dates, "close": closes})
        result = compute_vix_filter(vix_df, lookback=200)
        assert result["vix_filter"].iloc[198] == False  # noqa: E712
        assert result["vix_filter"].iloc[199] == False  # noqa: E712
        assert result["vix_filter"].iloc[-1] == True  # noqa: E712

    def test_boundary_equal(self):
        n = 200
        closes = [20.0] * n
        dates = pd.date_range("2020-01-01", periods=n, freq="B")
        vix_df = pd.DataFrame({"trade_date": dates, "close": closes})
        result = compute_vix_filter(vix_df, lookback=200)
        assert result["vix_filter"].iloc[-1] == False  # noqa: E712


class TestSimulateStrategy:
    def test_basic_equity_tracking(self):
        closes = np.array([100.0, 101.0, 102.0])
        opens_next = np.array([101.5, 102.5, 103.5])
        returns = np.log(opens_next / closes)
        mask = np.array([True, True, True])

        equity = simulate_strategy(
            returns, closes, opens_next, mask, capital=10_000, fee_fn=lambda e, p: 0
        )
        assert len(equity) == 4
        assert equity[0] == 10_000
        assert equity[1] == pytest.approx(10_150)

    def test_mask_skips_trades(self):
        closes = np.array([100.0, 100.0])
        opens_next = np.array([110.0, 110.0])
        returns = np.log(opens_next / closes)
        mask = np.array([False, True])

        equity = simulate_strategy(
            returns, closes, opens_next, mask, capital=10_000, fee_fn=lambda e, p: 0
        )
        assert equity[1] == 10_000
        assert equity[2] > 10_000

    def test_fees_reduce_equity(self):
        closes = np.array([100.0])
        opens_next = np.array([100.0])
        returns = np.log(opens_next / closes)
        mask = np.array([True])

        equity = simulate_strategy(returns, closes, opens_next, mask, capital=10_000)
        assert equity[1] < 10_000
