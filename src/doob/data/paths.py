"""Path resolution helpers for the warehouse data lake."""

from __future__ import annotations

from pathlib import Path

from doob.config import warehouse_root


def parquet_path_for_symbol(symbol: str, warehouse: Path | None = None) -> Path:
    """Return the canonical bronze parquet path for an equity ticker."""
    root = warehouse or warehouse_root()
    return (
        root / "data-lake" / "bronze" / "asset_class=equity" / f"symbol={symbol}" / "data.parquet"
    )
