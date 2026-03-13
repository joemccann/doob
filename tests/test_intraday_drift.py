"""Tests for the intraday drift backtesting engine.

All synthetic data — no file or network I/O.
"""

from __future__ import annotations

import numpy as np
import pandas as pd
import pytest

from doob.strategies.intraday_drift import (
    compute_intraday_returns,
    simulate_strategy,
)


def _spy_df(opens, closes):
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


class TestComputeIntradayReturns:
    def test_basic_log_returns(self):
        df = _spy_df([100, 102, 104], [101, 103, 105])
        result = compute_intraday_returns(df)
        np.testing.assert_almost_equal(result.iloc[0], np.log(101 / 100))
        np.testing.assert_almost_equal(result.iloc[1], np.log(103 / 102))
        np.testing.assert_almost_equal(result.iloc[2], np.log(105 / 104))

    def test_negative_return(self):
        df = _spy_df([105], [100])
        result = compute_intraday_returns(df)
        assert result.iloc[0] < 0

    def test_flat_day(self):
        df = _spy_df([100], [100])
        result = compute_intraday_returns(df)
        np.testing.assert_almost_equal(result.iloc[0], 0.0)


class TestSimulateStrategy:
    def test_basic_equity_tracking(self):
        opens = np.array([100.0, 101.0, 102.0])
        closes = np.array([101.0, 102.0, 103.0])
        mask = np.array([True, True, True])

        equity = simulate_strategy(opens, closes, mask, capital=10_000, fee_fn=lambda e, p: 0)
        assert len(equity) == 4
        assert equity[0] == 10_000
        assert equity[1] == pytest.approx(10_100)

    def test_mask_skips_trades(self):
        opens = np.array([100.0, 100.0])
        closes = np.array([110.0, 110.0])
        mask = np.array([False, True])

        equity = simulate_strategy(opens, closes, mask, capital=10_000, fee_fn=lambda e, p: 0)
        assert equity[1] == 10_000
        assert equity[2] > 10_000

    def test_loss_day(self):
        opens = np.array([100.0])
        closes = np.array([95.0])
        mask = np.array([True])

        equity = simulate_strategy(opens, closes, mask, capital=10_000, fee_fn=lambda e, p: 0)
        assert equity[1] == pytest.approx(9_500)

    def test_fees_reduce_equity(self):
        opens = np.array([100.0])
        closes = np.array([100.0])
        mask = np.array([True])

        equity = simulate_strategy(opens, closes, mask, capital=10_000)
        assert equity[1] < 10_000

    def test_short_direction(self):
        opens = np.array([100.0])
        closes = np.array([95.0])
        mask = np.array([True])

        equity = simulate_strategy(
            opens, closes, mask, capital=10_000, fee_fn=lambda e, p: 0, short=True
        )
        # Short: pnl = -1 * 100 * (95-100) = 500
        assert equity[1] == pytest.approx(10_500)
