"""Tests for the Nasdaq-100 SMA breadth analysis."""

from __future__ import annotations

import json
from pathlib import Path

import pandas as pd
import pytest

from doob.strategies.ndx100_sma_breadth import (
    analyze_breadth,
    build_histogram_table,
    compute_breadth,
    compute_forward_returns,
    compute_point_in_time_breadth,
    format_report,
    load_universe,
    select_trailing_sessions,
    summarize_conditioned_forward_returns,
    summarize_distribution,
)


def _close_frame() -> pd.DataFrame:
    dates = pd.bdate_range("2026-03-02", periods=8)
    frame = pd.DataFrame(
        {
            "AAA": [10, 10, 10, 10, 11, 12, 13, 14],
            "BBB": [10, 10, 10, 10, 9, 8, 7, 6],
            "CCC": [10, 10, 10, 10, 10, 10, 10, 10],
        },
        index=dates,
    )
    frame.index.name = "trade_date"
    return frame


def _write_symbol_parquet(warehouse: Path, symbol: str, closes: list[float]) -> None:
    path = warehouse / "data-lake" / "bronze" / "asset_class=equity" / f"symbol={symbol}"
    path.mkdir(parents=True, exist_ok=True)
    dates = pd.bdate_range("2026-03-02", periods=len(closes))
    frame = pd.DataFrame({"trade_date": dates, "close": closes})
    frame.to_parquet(path / "data.parquet", index=False)


class TestComputeBreadth:
    def test_counts_and_percentages(self):
        result = compute_breadth(_close_frame(), lookback=5)

        row = result.loc[result["trade_date"] == pd.Timestamp("2026-03-06")].iloc[0]
        assert row["eligible_count"] == 3
        assert row["above_count"] == 1
        assert row["below_or_equal_count"] == 2
        assert row["unavailable_count"] == 0
        assert row["pct_above"] == pytest.approx(100 / 3)
        assert row["pct_below_or_equal"] == pytest.approx(200 / 3)

    def test_universe_size_accounts_for_missing_symbols(self):
        result = compute_breadth(_close_frame()[["AAA", "BBB"]], lookback=5, universe_size=3)

        row = result.loc[result["trade_date"] == pd.Timestamp("2026-03-06")].iloc[0]
        assert row["eligible_count"] == 2
        assert row["above_count"] == 1
        assert row["below_or_equal_count"] == 1
        assert row["unavailable_count"] == 1
        assert row["pct_above"] == pytest.approx(50.0)


class TestSelectTrailingSessions:
    def test_returns_last_n_sessions_through_end_date(self):
        breadth = compute_breadth(_close_frame(), lookback=5)

        trailing = select_trailing_sessions(breadth, end_date="2026-03-11", sessions=3)
        assert trailing["trade_date"].tolist() == [
            pd.Timestamp("2026-03-09"),
            pd.Timestamp("2026-03-10"),
            pd.Timestamp("2026-03-11"),
        ]

    def test_requires_requested_end_date(self):
        breadth = compute_breadth(_close_frame(), lookback=5)

        with pytest.raises(ValueError, match="Requested end date 2026-03-12 is not present"):
            select_trailing_sessions(breadth, end_date="2026-03-12", sessions=3)


class TestPointInTimeBreadth:
    def test_uses_date_specific_membership(self):
        prices = _close_frame()[["AAA", "BBB", "CCC"]]
        memberships = {
            "2026-03-06": {"AAA", "BBB"},
            "2026-03-09": {"AAA", "CCC"},
            "2026-03-10": {"AAA", "CCC"},
            "2026-03-11": {"AAA", "CCC"},
        }

        result = compute_point_in_time_breadth(prices, memberships, lookback=5)

        row_first = result.loc[result["trade_date"] == pd.Timestamp("2026-03-06")].iloc[0]
        assert row_first["eligible_count"] == 2
        assert row_first["above_count"] == 1
        assert row_first["below_or_equal_count"] == 1
        assert row_first["unavailable_count"] == 0
        assert row_first["pct_above"] == pytest.approx(50.0)

        row_second = result.loc[result["trade_date"] == pd.Timestamp("2026-03-09")].iloc[0]
        assert row_second["eligible_count"] == 2
        assert row_second["above_count"] == 1
        assert row_second["below_or_equal_count"] == 1
        assert row_second["unavailable_count"] == 0
        assert row_second["pct_above"] == pytest.approx(50.0)


class TestAnalyzeBreadth:
    def test_end_to_end_with_temp_parquet(self, tmp_path: Path):
        warehouse = tmp_path / "warehouse"
        preset_path = tmp_path / "ndx100-test.json"

        _write_symbol_parquet(warehouse, "AAA", [10, 10, 10, 10, 11, 12, 13, 14])
        _write_symbol_parquet(warehouse, "BBB", [10, 10, 10, 10, 9, 8, 7, 6])
        preset_path.write_text(json.dumps({"tickers": ["AAA", "BBB", "CCC"]}))

        trailing, target_row, summary, histogram, missing = analyze_breadth(
            preset_path=preset_path,
            warehouse=warehouse,
            end_date="2026-03-11",
            sessions=4,
            lookback=5,
        )

        assert trailing["trade_date"].tolist() == [
            pd.Timestamp("2026-03-06"),
            pd.Timestamp("2026-03-09"),
            pd.Timestamp("2026-03-10"),
            pd.Timestamp("2026-03-11"),
        ]
        assert target_row["above_count"] == 1
        assert target_row["below_or_equal_count"] == 1
        assert target_row["unavailable_count"] == 1
        assert target_row["pct_above"] == pytest.approx(50.0)
        assert missing == ["CCC"]
        assert summary["observations"] == 4
        assert summary["mean"] == pytest.approx(50.0)
        assert summary["std"] == pytest.approx(0.0)

        histogram = histogram.set_index("breadth_band")
        assert histogram.loc["40-50%", "days"] == 4
        assert histogram["days"].sum() == 4


class TestForwardReturns:
    def test_compute_forward_returns(self):
        dates = pd.bdate_range("2026-03-02", periods=5)
        prices = pd.Series([100.0, 110.0, 121.0, 133.1, 146.41], index=dates)

        result = compute_forward_returns(prices, horizons={"1d": 1, "2d": 2})
        assert result.loc[dates[0], "1d"] == pytest.approx(0.10)
        assert result.loc[dates[0], "2d"] == pytest.approx(0.21)
        assert pd.isna(result.loc[dates[-1], "1d"])
        assert pd.isna(result.loc[dates[-2], "2d"])

    def test_summarize_conditioned_forward_returns(self):
        dates = pd.bdate_range("2026-03-02", periods=6)
        breadth = pd.DataFrame(
            {
                "trade_date": dates,
                "pct_below_or_equal": [70.0, 60.0, 80.0, 50.0, 72.0, 40.0],
            }
        )
        closes = pd.DataFrame(
            {
                "SPY": [100.0, 110.0, 121.0, 133.1, 146.41, 161.051],
                "SPXL": [50.0, 60.0, 72.0, 86.4, 103.68, 124.416],
            },
            index=dates,
        )

        triggered, summary = summarize_conditioned_forward_returns(
            breadth,
            closes,
            min_pct_below=65.0,
            horizons={"1d": 1, "2d": 2},
        )

        assert triggered["trade_date"].tolist() == [
            pd.Timestamp("2026-03-02"),
            pd.Timestamp("2026-03-04"),
            pd.Timestamp("2026-03-06"),
        ]

        spy_1d = summary[(summary["asset"] == "SPY") & (summary["horizon"] == "1d")].iloc[0]
        assert spy_1d["signals"] == 3
        assert spy_1d["observations"] == 3
        assert spy_1d["mean_return_pct"] == pytest.approx(10.0)
        assert spy_1d["median_return_pct"] == pytest.approx(10.0)
        assert spy_1d["positive_rate_pct"] == pytest.approx(100.0)

        spxl_2d = summary[(summary["asset"] == "SPXL") & (summary["horizon"] == "2d")].iloc[0]
        assert spxl_2d["signals"] == 3
        assert spxl_2d["observations"] == 2
        assert spxl_2d["mean_return_pct"] == pytest.approx(44.0)
        assert spxl_2d["median_return_pct"] == pytest.approx(44.0)


class TestLoadUniverse:
    def test_loads_from_preset(self, tmp_path: Path):
        preset = tmp_path / "test.json"
        preset.write_text(json.dumps({"tickers": ["aapl", "msft"]}))
        result = load_universe(preset)
        assert result == ["AAPL", "MSFT"]

    def test_empty_raises(self, tmp_path: Path):
        preset = tmp_path / "empty.json"
        preset.write_text(json.dumps({"tickers": []}))
        with pytest.raises(ValueError, match="non-empty ticker list"):
            load_universe(preset)


class TestSummarizeDistribution:
    def test_basic_stats(self):
        series = pd.Series([10.0, 20.0, 30.0, 40.0, 50.0])
        result = summarize_distribution(series)
        assert result["observations"] == 5
        assert result["mean"] == pytest.approx(30.0)
        assert result["min"] == pytest.approx(10.0)
        assert result["max"] == pytest.approx(50.0)
        assert result["median"] == pytest.approx(30.0)

    def test_empty_raises(self):
        with pytest.raises(ValueError, match="empty series"):
            summarize_distribution(pd.Series(dtype=float))

    def test_single_value(self):
        result = summarize_distribution(pd.Series([42.0]))
        assert result["observations"] == 1
        assert result["std"] == pytest.approx(0.0)


class TestBuildHistogramTable:
    def test_basic_histogram(self):
        series = pd.Series([5.0, 15.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0, 85.0, 95.0])
        hist = build_histogram_table(series, bin_size=10)
        assert len(hist) == 10
        assert hist["days"].sum() == 10
        assert "breadth_band" in hist.columns
        assert "share_of_days_pct" in hist.columns

    def test_bad_bin_size_raises(self):
        with pytest.raises(ValueError, match="divide 100 evenly"):
            build_histogram_table(pd.Series([50.0]), bin_size=7)

    def test_empty_raises(self):
        with pytest.raises(ValueError, match="empty series"):
            build_histogram_table(pd.Series(dtype=float))


class TestFormatReport:
    def test_report_output(self, tmp_path: Path):
        warehouse = tmp_path / "warehouse"
        preset_path = tmp_path / "ndx100-test.json"
        _write_symbol_parquet(warehouse, "AAA", [10, 10, 10, 10, 11, 12, 13, 14])
        _write_symbol_parquet(warehouse, "BBB", [10, 10, 10, 10, 9, 8, 7, 6])
        preset_path.write_text(json.dumps({"tickers": ["AAA", "BBB", "CCC"]}))

        trailing, target_row, summary, histogram, missing = analyze_breadth(
            preset_path=preset_path,
            warehouse=warehouse,
            end_date="2026-03-11",
            sessions=4,
            lookback=5,
        )

        report = format_report(
            trailing=trailing,
            target_row=target_row,
            summary=summary,
            histogram=histogram,
            universe_size=3,
            missing=missing,
            lookback=5,
        )
        assert "NASDAQ-100 Breadth Report" in report
        assert "Universe size: 3" in report
        assert "Missing parquet symbols: 1 (CCC)" in report
        assert "Breadth histogram" in report


class TestComputeBreadthEdgeCases:
    def test_negative_lookback_raises(self):
        with pytest.raises(ValueError, match="lookback must be positive"):
            compute_breadth(_close_frame(), lookback=-1)

    def test_small_universe_size_raises(self):
        with pytest.raises(ValueError, match="universe_size cannot be smaller"):
            compute_breadth(_close_frame(), lookback=5, universe_size=1)


class TestPointInTimeBreadthEdgeCases:
    def test_empty_prices_raises(self):
        with pytest.raises(ValueError, match="prices must be non-empty"):
            compute_point_in_time_breadth(pd.DataFrame(), {}, lookback=5)

    def test_empty_memberships_raises(self):
        with pytest.raises(ValueError, match="memberships must be non-empty"):
            compute_point_in_time_breadth(_close_frame(), {}, lookback=5)

    def test_negative_lookback_raises(self):
        with pytest.raises(ValueError, match="lookback must be positive"):
            compute_point_in_time_breadth(_close_frame(), {"2026-03-06": {"AAA"}}, lookback=-1)


class TestSelectTrailingSessionsEdgeCases:
    def test_negative_sessions_raises(self):
        breadth = compute_breadth(_close_frame(), lookback=5)
        with pytest.raises(ValueError, match="sessions must be positive"):
            select_trailing_sessions(breadth, end_date="2026-03-11", sessions=-1)

    def test_empty_breadth_raises(self):
        empty_breadth = pd.DataFrame({"trade_date": [], "eligible_count": []})
        with pytest.raises(ValueError, match="No eligible breadth"):
            select_trailing_sessions(empty_breadth, end_date="2026-03-11")


class TestComputeForwardReturnsEdgeCases:
    def test_default_horizons(self):
        dates = pd.bdate_range("2026-03-02", periods=100)
        prices = pd.Series(range(100, 200), index=dates, dtype=float)
        result = compute_forward_returns(prices)
        assert "1d" in result.columns
        assert "1w" in result.columns

    def test_empty_horizons_raises(self):
        dates = pd.bdate_range("2026-03-02", periods=5)
        prices = pd.Series([100.0, 110.0, 121.0, 133.1, 146.41], index=dates)
        with pytest.raises(ValueError, match="non-empty"):
            compute_forward_returns(prices, horizons={})

    def test_negative_horizon_raises(self):
        dates = pd.bdate_range("2026-03-02", periods=5)
        prices = pd.Series([100.0, 110.0, 121.0, 133.1, 146.41], index=dates)
        with pytest.raises(ValueError, match="positive"):
            compute_forward_returns(prices, horizons={"bad": -1})


class TestSummarizeConditionedEdgeCases:
    def test_empty_trigger_returns_empty_summary(self):
        dates = pd.bdate_range("2026-03-02", periods=3)
        breadth = pd.DataFrame(
            {
                "trade_date": dates,
                "pct_below_or_equal": [10.0, 20.0, 30.0],
            }
        )
        closes = pd.DataFrame(
            {"SPY": [100.0, 101.0, 102.0]},
            index=dates,
        )
        triggered, summary = summarize_conditioned_forward_returns(
            breadth, closes, min_pct_below=99.0
        )
        assert triggered.empty
        assert summary.empty

    def test_invalid_threshold_raises(self):
        with pytest.raises(ValueError, match="min_pct_below"):
            summarize_conditioned_forward_returns(
                pd.DataFrame({"trade_date": [], "pct_below_or_equal": []}),
                pd.DataFrame({"SPY": []}),
                min_pct_below=150.0,
            )

    def test_series_input(self):
        dates = pd.bdate_range("2026-03-02", periods=3)
        breadth = pd.DataFrame(
            {"trade_date": dates, "pct_below_or_equal": [70.0, 80.0, 90.0]}
        )
        closes = pd.Series([100.0, 110.0, 121.0], index=dates, name="SPY")
        triggered, summary = summarize_conditioned_forward_returns(
            breadth, closes, min_pct_below=65.0, horizons={"1d": 1}
        )
        assert len(triggered) == 3
        assert "SPY" in summary["asset"].values
