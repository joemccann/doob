"""Data loaders for parquet, DuckDB, and external sources."""

from __future__ import annotations

import time
from pathlib import Path

import duckdb
import pandas as pd

from doob.config import warehouse_root
from doob.data.paths import parquet_path_for_symbol


def load_ticker_ohlcv(
    ticker: str = "SPY",
    parquet_path: Path | None = None,
    warehouse: Path | None = None,
) -> pd.DataFrame:
    """Load daily OHLCV bars from bronze parquet via DuckDB.

    If ``parquet_path`` is given it should point to the symbol directory
    (e.g. ``…/symbol=SPY``).  Otherwise the path is resolved from
    ``warehouse`` (or the default warehouse).
    """
    if parquet_path is not None:
        data_file = parquet_path / "data.parquet"
    else:
        root = warehouse or warehouse_root()
        data_file = parquet_path_for_symbol(ticker, warehouse=root)

    if not data_file.exists():
        raise FileNotFoundError(f"{ticker} parquet not found: {data_file}")

    conn = duckdb.connect(":memory:")
    df = conn.execute(
        f"SELECT trade_date, open, high, low, close, volume "
        f"FROM read_parquet('{data_file}') ORDER BY trade_date"
    ).fetchdf()
    conn.close()
    df["trade_date"] = pd.to_datetime(df["trade_date"])
    return df


def load_close_frame(
    symbols: list[str],
    warehouse: Path | None = None,
    start_date: str | pd.Timestamp | None = None,
    end_date: str | pd.Timestamp | None = None,
) -> tuple[pd.DataFrame, list[str]]:
    """Load a trade_date x symbol close-price matrix from bronze parquet."""
    root = warehouse or warehouse_root()
    start_ts = pd.Timestamp(start_date) if start_date is not None else None
    end_ts = pd.Timestamp(end_date) if end_date is not None else None

    series_by_symbol: dict[str, pd.Series] = {}
    missing: list[str] = []

    for symbol in symbols:
        data_file = parquet_path_for_symbol(symbol, warehouse=root)
        if not data_file.exists():
            missing.append(symbol)
            continue

        frame = pd.read_parquet(data_file, columns=["trade_date", "close"])
        frame["trade_date"] = pd.to_datetime(frame["trade_date"])
        if start_ts is not None:
            frame = frame[frame["trade_date"] >= start_ts]
        if end_ts is not None:
            frame = frame[frame["trade_date"] <= end_ts]
        frame = frame.drop_duplicates(subset=["trade_date"], keep="last").sort_values("trade_date")

        if frame.empty:
            missing.append(symbol)
            continue

        series_by_symbol[symbol] = frame.set_index("trade_date")["close"].astype(float)

    if not series_by_symbol:
        return pd.DataFrame(), missing

    prices = pd.DataFrame(series_by_symbol).sort_index()
    prices.index.name = "trade_date"
    return prices, missing


def load_vix_from_cboe(
    url: str = "https://cdn.cboe.com/api/global/us_indices/daily_prices/VIX_History.csv",
    cache_path: Path | None = None,
    stale_seconds: int = 86400,
    _opener=None,
) -> pd.DataFrame:
    """Download/cache CBOE VIX CSV and return DataFrame.

    Re-downloads if cache is older than ``stale_seconds``.
    """
    if cache_path is None:
        root = warehouse_root()
        cache_path = root / "data-lake" / "bronze" / "external" / "vix_cboe_history.csv"

    cache_path = Path(cache_path)
    need_download = True
    if cache_path.exists():
        age = time.time() - cache_path.stat().st_mtime
        if age < stale_seconds:
            need_download = False

    if need_download:
        import urllib.request

        cache_path.parent.mkdir(parents=True, exist_ok=True)
        opener = _opener or urllib.request.urlopen
        resp = opener(url)
        data = resp.read()
        if isinstance(data, bytes):
            data = data.decode("utf-8")
        cache_path.write_text(data)

    df = pd.read_csv(cache_path)
    df.columns = [c.strip().lower() for c in df.columns]
    df = df.rename(columns={"date": "trade_date"})
    df["trade_date"] = pd.to_datetime(df["trade_date"])
    df = df.sort_values("trade_date").reset_index(drop=True)
    return df
