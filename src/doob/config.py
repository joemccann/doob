"""Centralized configuration for the doob backtesting package.

Resolution order for warehouse path:
    DOOB_WAREHOUSE_PATH env var -> .env file -> ~/market-warehouse (default)
"""

from __future__ import annotations

import os
from pathlib import Path


_DEFAULT_WAREHOUSE = Path.home() / "market-warehouse"
_PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent


def _load_dotenv_value(key: str) -> str | None:
    """Read a key from the project-root .env file, if present."""
    env_file = _PROJECT_ROOT / ".env"
    if not env_file.exists():
        return None
    for line in env_file.read_text().splitlines():
        line = line.strip()
        if line.startswith("#") or "=" not in line:
            continue
        k, _, v = line.partition("=")
        if k.strip() == key:
            return v.strip().strip("'\"")
    return None


def warehouse_root() -> Path:
    """Resolved warehouse path. Fails fast if the path is invalid."""
    env_val = os.environ.get("DOOB_WAREHOUSE_PATH") or _load_dotenv_value("DOOB_WAREHOUSE_PATH")
    root = Path(env_val) if env_val else _DEFAULT_WAREHOUSE
    if not root.exists():
        raise FileNotFoundError(f"Warehouse not found: {root}")
    bronze = root / "data-lake" / "bronze"
    if not bronze.exists():
        raise FileNotFoundError(f"Warehouse missing expected data-lake/bronze/ structure: {root}")
    return root


def bronze_equity_dir(warehouse: Path | None = None) -> Path:
    """Bronze parquet root for equities."""
    root = warehouse or warehouse_root()
    return root / "data-lake" / "bronze" / "asset_class=equity"


def output_dir() -> Path:
    """Output root for generated artifacts."""
    return _PROJECT_ROOT / "output"


def presets_dir() -> Path:
    """Presets root directory."""
    return _PROJECT_ROOT / "presets"
