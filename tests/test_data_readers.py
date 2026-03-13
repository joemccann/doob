"""Tests for doob.data.readers."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock

import pytest

from tests.conftest import write_symbol_parquet
from doob.data.readers import load_close_frame, load_ticker_ohlcv, load_vix_from_cboe


class TestLoadTickerOhlcv:
    def test_loads_from_parquet(self, tmp_warehouse: Path):
        write_symbol_parquet(tmp_warehouse, "SPY", [100.0, 101.0, 102.0])
        parquet_path = (
            tmp_warehouse / "data-lake" / "bronze" / "asset_class=equity" / "symbol=SPY"
        )

        df = load_ticker_ohlcv("SPY", parquet_path=parquet_path)
        assert len(df) == 3
        assert "trade_date" in df.columns
        assert "close" in df.columns
        assert df["close"].iloc[0] == pytest.approx(100.0)

    def test_loads_from_warehouse(self, tmp_warehouse: Path):
        write_symbol_parquet(tmp_warehouse, "AAPL", [150.0, 151.0])

        df = load_ticker_ohlcv("AAPL", warehouse=tmp_warehouse)
        assert len(df) == 2
        assert df["close"].iloc[0] == pytest.approx(150.0)

    def test_missing_file_raises(self, tmp_path: Path):
        with pytest.raises(FileNotFoundError):
            load_ticker_ohlcv("FAKE", parquet_path=tmp_path / "nonexistent")


class TestLoadCloseFrame:
    def test_loads_multiple_symbols(self, tmp_warehouse: Path):
        write_symbol_parquet(tmp_warehouse, "AAA", [10.0, 11.0, 12.0])
        write_symbol_parquet(tmp_warehouse, "BBB", [20.0, 21.0, 22.0])

        prices, missing = load_close_frame(
            ["AAA", "BBB", "CCC"], warehouse=tmp_warehouse
        )
        assert "AAA" in prices.columns
        assert "BBB" in prices.columns
        assert missing == ["CCC"]
        assert len(prices) == 3

    def test_date_filtering(self, tmp_warehouse: Path):
        write_symbol_parquet(tmp_warehouse, "AAA", [10.0, 11.0, 12.0, 13.0, 14.0])

        prices, _ = load_close_frame(
            ["AAA"],
            warehouse=tmp_warehouse,
            start_date="2026-03-04",
            end_date="2026-03-05",
        )
        assert len(prices) == 2

    def test_empty_returns_empty(self, tmp_warehouse: Path):
        prices, missing = load_close_frame(["NOPE"], warehouse=tmp_warehouse)
        assert prices.empty
        assert missing == ["NOPE"]


class TestLoadVixFromCboe:
    def test_mock_download(self, tmp_path: Path):
        csv_content = (
            " DATE, OPEN, HIGH, LOW, CLOSE\n"
            "01/02/2020, 13.50, 14.00, 13.00, 13.78\n"
            "01/03/2020, 14.00, 14.50, 13.50, 14.02\n"
        )

        mock_resp = MagicMock()
        mock_resp.read.return_value = csv_content.encode("utf-8")
        mock_opener = MagicMock(return_value=mock_resp)

        cache = tmp_path / "vix.csv"
        df = load_vix_from_cboe(
            url="http://fake", cache_path=cache, _opener=mock_opener
        )

        assert len(df) == 2
        assert "trade_date" in df.columns
        assert "close" in df.columns
        assert df["close"].iloc[0] == pytest.approx(13.78)
        mock_opener.assert_called_once_with("http://fake")

    def test_uses_cache(self, tmp_path: Path):
        csv_content = "DATE,OPEN,HIGH,LOW,CLOSE\n01/02/2020,13.50,14.00,13.00,13.78\n"
        cache = tmp_path / "vix.csv"
        cache.write_text(csv_content)

        mock_opener = MagicMock()
        df = load_vix_from_cboe(
            url="http://fake",
            cache_path=cache,
            stale_seconds=999999,
            _opener=mock_opener,
        )

        assert len(df) == 1
        mock_opener.assert_not_called()
