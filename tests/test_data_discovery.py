"""Tests for doob.data.discovery."""

from __future__ import annotations

from pathlib import Path

import pytest

from tests.conftest import write_symbol_parquet
from doob.data.discovery import discover_symbols


class TestDiscoverSymbols:
    def test_discovers_symbols(self, tmp_warehouse: Path):
        write_symbol_parquet(tmp_warehouse, "AAPL", [100.0, 101.0])
        write_symbol_parquet(tmp_warehouse, "MSFT", [200.0, 201.0])
        bronze = tmp_warehouse / "data-lake" / "bronze" / "asset_class=equity"

        result = discover_symbols(bronze_dir=bronze)
        assert result == ["AAPL", "MSFT"]

    def test_skips_dirs_without_parquet(self, tmp_warehouse: Path):
        bronze = tmp_warehouse / "data-lake" / "bronze" / "asset_class=equity"
        (bronze / "symbol=EMPTY").mkdir(parents=True)
        write_symbol_parquet(tmp_warehouse, "AAPL", [100.0])

        result = discover_symbols(bronze_dir=bronze)
        assert result == ["AAPL"]

    def test_empty_bronze_returns_empty(self, tmp_warehouse: Path):
        bronze = tmp_warehouse / "data-lake" / "bronze" / "asset_class=equity"
        result = discover_symbols(bronze_dir=bronze)
        assert result == []

    def test_missing_dir_raises(self, tmp_path: Path):
        with pytest.raises(FileNotFoundError):
            discover_symbols(bronze_dir=tmp_path / "nonexistent")
