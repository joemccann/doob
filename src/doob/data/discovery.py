"""Symbol discovery from the bronze parquet layer."""

from __future__ import annotations

from pathlib import Path

from doob.config import bronze_equity_dir


def discover_symbols(bronze_dir: Path | str | None = None) -> list[str]:
    """Return all symbols currently stored in the canonical bronze layer.

    Scans for ``symbol=<TICKER>`` directories containing ``data.parquet``.
    """
    if bronze_dir is not None:
        root = Path(bronze_dir)
    else:
        root = bronze_equity_dir()

    if not root.exists():
        raise FileNotFoundError(f"Bronze equity directory not found: {root}")

    symbols: list[str] = []
    for child in sorted(root.iterdir()):
        if child.is_dir() and child.name.startswith("symbol="):
            ticker = child.name.split("=", 1)[1]
            if (child / "data.parquet").exists():
                symbols.append(ticker)
    return symbols
