"""Shared test fixtures for the doob package."""

from __future__ import annotations

from pathlib import Path

import pandas as pd
import pytest


@pytest.fixture()
def tmp_warehouse(tmp_path: Path) -> Path:
    """Create a minimal warehouse directory structure with synthetic parquet."""
    warehouse = tmp_path / "warehouse"
    bronze = warehouse / "data-lake" / "bronze" / "asset_class=equity"
    bronze.mkdir(parents=True)
    return warehouse


def write_symbol_parquet(
    warehouse: Path,
    symbol: str,
    closes: list[float],
    start_date: str = "2026-03-02",
) -> Path:
    """Write a minimal parquet file for a symbol in the bronze layer."""
    path = warehouse / "data-lake" / "bronze" / "asset_class=equity" / f"symbol={symbol}"
    path.mkdir(parents=True, exist_ok=True)
    dates = pd.bdate_range(start_date, periods=len(closes))
    opens = [c - 0.5 for c in closes]
    highs = [c + 1.0 for c in closes]
    lows = [c - 1.0 for c in closes]
    frame = pd.DataFrame(
        {
            "trade_date": dates,
            "open": opens,
            "high": highs,
            "low": lows,
            "close": closes,
            "volume": [1_000_000] * len(closes),
        }
    )
    out = path / "data.parquet"
    frame.to_parquet(out, index=False)
    return out
