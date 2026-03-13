"""NASDAQ-100 5-day SMA breadth analysis.

Computes, for each trading session, how many Nasdaq-100 members closed above
their 5-day simple moving average and summarizes the distribution of that daily
breadth percentage over a trailing window.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import pandas as pd
import requests
from requests.exceptions import RequestException

from doob.config import output_dir
from doob.data.readers import load_close_frame

DEFAULT_FORWARD_HORIZONS = {
    "1d": 1,
    "1w": 5,
    "1m": 21,
    "3m": 63,
}
NASDAQ_WEIGHTING_URL = "https://indexes.nasdaqomx.com/Index/WeightingData"
YAHOO_CHART_URL = "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}"
DEFAULT_HTTP_HEADERS = {"User-Agent": "Mozilla/5.0", "Accept": "application/json,text/csv,*/*"}

OUTPUT_DIR = output_dir()
DEFAULT_PRESET = Path(__file__).resolve().parent.parent.parent.parent / "presets" / "ndx100.json"
WAREHOUSE = Path.home() / "market-warehouse"


def load_universe(preset_path: Path = DEFAULT_PRESET) -> list[str]:
    """Load the ticker universe from a preset JSON file."""
    payload = json.loads(Path(preset_path).read_text())
    tickers = payload.get("tickers")
    if not isinstance(tickers, list) or not tickers:
        raise ValueError(f"Preset {preset_path} does not contain a non-empty ticker list")
    return [str(ticker).upper() for ticker in tickers]


def compute_breadth(prices: pd.DataFrame, lookback: int = 5, universe_size: int | None = None) -> pd.DataFrame:
    """Compute daily breadth counts and percentages from a close-price matrix."""
    if lookback <= 0:
        raise ValueError("lookback must be positive")
    if universe_size is None:
        universe_size = prices.shape[1]
    if universe_size < prices.shape[1]:
        raise ValueError("universe_size cannot be smaller than the loaded price matrix width")

    rolling_sma = prices.rolling(window=lookback, min_periods=lookback).mean()
    eligible = prices.notna() & rolling_sma.notna()
    above = eligible & prices.gt(rolling_sma)
    below_or_equal = eligible & ~prices.gt(rolling_sma)

    result = pd.DataFrame(index=prices.index.copy())
    result["eligible_count"] = eligible.sum(axis=1).astype(int)
    result["above_count"] = above.sum(axis=1).astype(int)
    result["below_or_equal_count"] = below_or_equal.sum(axis=1).astype(int)
    result["unavailable_count"] = universe_size - result["eligible_count"]

    eligible_float = result["eligible_count"].replace(0, np.nan).astype(float)
    result["pct_above"] = (result["above_count"] / eligible_float) * 100.0
    result["pct_below_or_equal"] = (result["below_or_equal_count"] / eligible_float) * 100.0

    return result.reset_index()


def compute_point_in_time_breadth(
    prices: pd.DataFrame,
    memberships: dict[pd.Timestamp | str, set[str] | list[str] | tuple[str, ...]],
    lookback: int = 5,
) -> pd.DataFrame:
    """Compute breadth using a date-specific membership set for each session."""
    if lookback <= 0:
        raise ValueError("lookback must be positive")
    if prices.empty:
        raise ValueError("prices must be non-empty")
    if not memberships:
        raise ValueError("memberships must be non-empty")

    normalized_memberships: dict[pd.Timestamp, set[str]] = {
        pd.Timestamp(trade_date): {str(symbol).upper() for symbol in symbols}
        for trade_date, symbols in memberships.items()
    }

    rolling_sma = prices.rolling(window=lookback, min_periods=lookback).mean()
    results: list[dict[str, float | int | pd.Timestamp]] = []
    columns = pd.Index(prices.columns.astype(str).str.upper())

    for trade_date in prices.index:
        members = normalized_memberships.get(pd.Timestamp(trade_date))
        if members is None:
            continue

        membership_mask = columns.isin(members)
        row_prices = prices.loc[trade_date]
        row_sma = rolling_sma.loc[trade_date]
        eligible = membership_mask & row_prices.notna().to_numpy() & row_sma.notna().to_numpy()
        above = eligible & row_prices.gt(row_sma).to_numpy()

        universe_size = len(members)
        eligible_count = int(eligible.sum())
        above_count = int(above.sum())
        below_count = eligible_count - above_count
        unavailable_count = universe_size - eligible_count

        pct_above = np.nan if eligible_count == 0 else (above_count / eligible_count) * 100.0
        pct_below = np.nan if eligible_count == 0 else (below_count / eligible_count) * 100.0

        results.append(
            {
                "trade_date": pd.Timestamp(trade_date),
                "eligible_count": eligible_count,
                "above_count": above_count,
                "below_or_equal_count": below_count,
                "unavailable_count": unavailable_count,
                "pct_above": pct_above,
                "pct_below_or_equal": pct_below,
            }
        )

    return pd.DataFrame(results)


def select_trailing_sessions(
    breadth: pd.DataFrame,
    end_date: str | pd.Timestamp,
    sessions: int = 252,
) -> pd.DataFrame:
    """Return the trailing N sessions through the requested end date."""
    if sessions <= 0:
        raise ValueError("sessions must be positive")

    end_ts = pd.Timestamp(end_date)
    if breadth.empty:
        raise ValueError(f"No eligible breadth observations on or before {end_ts.date()}")
    filtered = breadth[(breadth["trade_date"] <= end_ts) & (breadth["eligible_count"] > 0)].copy()
    if filtered.empty:
        raise ValueError(f"No eligible breadth observations on or before {end_ts.date()}")
    if filtered["trade_date"].iloc[-1] != end_ts:
        latest = filtered["trade_date"].iloc[-1].date()
        raise ValueError(f"Requested end date {end_ts.date()} is not present in the data; latest available date is {latest}")
    return filtered.tail(sessions).reset_index(drop=True)


def summarize_distribution(series: pd.Series) -> dict[str, float | int]:
    """Summarize a breadth-percentage series with common distribution stats."""
    clean = pd.Series(series, dtype=float).dropna()
    if clean.empty:
        raise ValueError("Cannot summarize an empty series")

    quantiles = clean.quantile([0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.95])
    std = clean.std(ddof=1) if len(clean) > 1 else 0.0

    return {
        "observations": int(clean.size),
        "mean": float(clean.mean()),
        "std": float(std),
        "min": float(clean.min()),
        "p05": float(quantiles.loc[0.05]),
        "p10": float(quantiles.loc[0.10]),
        "p25": float(quantiles.loc[0.25]),
        "median": float(quantiles.loc[0.50]),
        "p75": float(quantiles.loc[0.75]),
        "p90": float(quantiles.loc[0.90]),
        "p95": float(quantiles.loc[0.95]),
        "max": float(clean.max()),
    }


def build_histogram_table(series: pd.Series, bin_size: int = 10) -> pd.DataFrame:
    """Bucket daily breadth percentages into equal-width bands."""
    if 100 % bin_size != 0:
        raise ValueError("bin_size must divide 100 evenly")

    clean = pd.Series(series, dtype=float).dropna()
    if clean.empty:
        raise ValueError("Cannot build a histogram from an empty series")

    edges = np.arange(0, 100 + bin_size, bin_size, dtype=float)
    labels = [f"{int(left)}-{int(right)}%" for left, right in zip(edges[:-1], edges[1:])]
    categories = pd.cut(clean, bins=edges, labels=labels, include_lowest=True, right=True, ordered=True)
    counts = categories.value_counts().reindex(labels, fill_value=0)

    histogram = pd.DataFrame({"breadth_band": labels, "days": counts.to_numpy(dtype=int)})
    histogram["share_of_days_pct"] = (histogram["days"] / len(clean)) * 100.0
    return histogram


def compute_forward_returns(
    close_series: pd.Series,
    horizons: dict[str, int] | None = None,
) -> pd.DataFrame:
    """Compute simple close-to-close forward returns for each named horizon."""
    if horizons is None:
        horizons = DEFAULT_FORWARD_HORIZONS
    if not horizons:
        raise ValueError("horizons must be non-empty")

    clean = pd.Series(close_series, dtype=float).sort_index()
    result = pd.DataFrame(index=clean.index.copy())

    for label, steps in horizons.items():
        if steps <= 0:
            raise ValueError("forward-return horizons must be positive")
        result[label] = (clean.shift(-steps) / clean) - 1.0

    result.index.name = clean.index.name
    return result


def fetch_nasdaq_memberships(  # pragma: no cover
    trade_dates: list[pd.Timestamp] | pd.DatetimeIndex | pd.Series,
    symbol: str = "NDX",
    time_of_day: str = "EOD",
    session: requests.Session | None = None,
    pause_seconds: float = 0.0,
) -> dict[pd.Timestamp, set[str]]:
    """Fetch official Nasdaq index memberships for specific trade dates."""
    http = session or requests.Session()
    if "User-Agent" not in http.headers:
        http.headers.update(DEFAULT_HTTP_HEADERS)

    memberships: dict[pd.Timestamp, set[str]] = {}
    for trade_date in pd.to_datetime(list(trade_dates)):
        response = http.post(
            NASDAQ_WEIGHTING_URL,
            data={
                "id": symbol,
                "tradeDate": f"{trade_date.date().isoformat()}T00:00:00.000",
                "timeOfDay": time_of_day,
            },
            timeout=20,
        )
        response.raise_for_status()
        payload = response.json()
        memberships[pd.Timestamp(trade_date)] = {
            str(row["Symbol"]).upper() for row in payload.get("aaData", []) if row.get("Symbol")
        }
        if pause_seconds > 0:
            time.sleep(pause_seconds)

    return memberships


def fetch_yahoo_daily_series(  # pragma: no cover
    symbol: str,
    start_date: str | pd.Timestamp,
    end_date: str | pd.Timestamp,
    adjusted: bool = False,
    session: requests.Session | None = None,
) -> pd.Series:
    """Fetch Yahoo daily close or adjusted-close data as a Series."""
    start_ts = pd.Timestamp(start_date)
    end_ts = pd.Timestamp(end_date)
    http = session or requests.Session()
    if "User-Agent" not in http.headers:
        http.headers.update(DEFAULT_HTTP_HEADERS)

    try:
        response = http.get(
            YAHOO_CHART_URL.format(symbol=symbol.upper()),
            params={
                "period1": int(start_ts.tz_localize("UTC").timestamp()),
                "period2": int((end_ts + pd.Timedelta(days=1)).tz_localize("UTC").timestamp()),
                "interval": "1d",
                "includeAdjustedClose": "true",
                "events": "div,splits",
            },
            timeout=20,
        )
        response.raise_for_status()
    except RequestException:
        return pd.Series(dtype=float, name=symbol.upper())
    payload = response.json()
    chart = payload.get("chart", {})
    if chart.get("error") is not None or not chart.get("result"):
        return pd.Series(dtype=float, name=symbol.upper())
    result = chart["result"][0]
    timestamps = result.get("timestamp", [])
    if not timestamps:
        return pd.Series(dtype=float, name=symbol.upper())

    if adjusted:
        values = result["indicators"]["adjclose"][0]["adjclose"]
    else:
        values = result["indicators"]["quote"][0]["close"]

    series = pd.Series(values, index=pd.to_datetime(timestamps, unit="s").normalize(), dtype=float, name=symbol.upper())
    series = series[(series.index >= start_ts.normalize()) & (series.index <= end_ts.normalize())]
    return series.dropna()


def summarize_conditioned_forward_returns(
    breadth: pd.DataFrame,
    asset_closes: pd.DataFrame | pd.Series,
    min_pct_below: float = 65.0,
    horizons: dict[str, int] | None = None,
) -> tuple[pd.DataFrame, pd.DataFrame]:
    """Summarize forward returns on dates where breadth crosses the threshold."""
    if not 0 <= min_pct_below <= 100:
        raise ValueError("min_pct_below must be between 0 and 100")
    if "trade_date" not in breadth or "pct_below_or_equal" not in breadth:
        raise ValueError("breadth must contain trade_date and pct_below_or_equal columns")

    if isinstance(asset_closes, pd.Series):
        asset_frame = asset_closes.to_frame(name=asset_closes.name or "asset")
    else:
        asset_frame = asset_closes.copy()
    if asset_frame.empty:
        raise ValueError("asset_closes must be non-empty")

    asset_frame = asset_frame.sort_index()
    breadth_frame = breadth.copy()
    breadth_frame["trade_date"] = pd.to_datetime(breadth_frame["trade_date"])
    triggered = breadth_frame.loc[breadth_frame["pct_below_or_equal"] >= min_pct_below].copy().reset_index(drop=True)
    trigger_dates = triggered["trade_date"]
    if trigger_dates.empty:
        return triggered, pd.DataFrame(
            columns=[
                "asset",
                "horizon",
                "signals",
                "observations",
                "mean_return_pct",
                "median_return_pct",
                "positive_rate_pct",
            ]
        )

    rows: list[dict[str, str | int | float]] = []
    for asset in asset_frame.columns:
        forward = compute_forward_returns(asset_frame[asset], horizons=horizons)
        triggered_forward = forward.reindex(trigger_dates).dropna(how="all")
        for horizon in forward.columns:
            realized = triggered_forward[horizon].dropna()
            observations = int(realized.size)
            if observations == 0:
                mean_return = np.nan
                median_return = np.nan
                positive_rate = np.nan
            else:
                mean_return = float(realized.mean() * 100.0)
                median_return = float(realized.median() * 100.0)
                positive_rate = float((realized > 0).mean() * 100.0)

            rows.append(
                {
                    "asset": str(asset),
                    "horizon": str(horizon),
                    "signals": int(trigger_dates.size),
                    "observations": observations,
                    "mean_return_pct": mean_return,
                    "median_return_pct": median_return,
                    "positive_rate_pct": positive_rate,
                }
            )

    return triggered, pd.DataFrame(rows)


def analyze_breadth(
    preset_path: Path = DEFAULT_PRESET,
    warehouse: Path = WAREHOUSE,
    end_date: str = "2026-03-11",
    sessions: int = 252,
    lookback: int = 5,
) -> tuple[pd.DataFrame, pd.Series, dict[str, float | int], pd.DataFrame, list[str]]:
    """Run the breadth analysis for a preset universe."""
    tickers = load_universe(preset_path)

    start_ts = pd.Timestamp(end_date) - pd.Timedelta(days=max(lookback * 10, sessions * 2))
    prices, missing = load_close_frame(
        tickers,
        warehouse=warehouse,
        start_date=start_ts.normalize(),
        end_date=pd.Timestamp(end_date),
    )
    if prices.empty:
        raise ValueError("No price data loaded for the requested universe")

    breadth = compute_breadth(prices, lookback=lookback, universe_size=len(tickers))
    trailing = select_trailing_sessions(breadth, end_date=end_date, sessions=sessions)
    target_row = trailing.loc[trailing["trade_date"] == pd.Timestamp(end_date)].iloc[0]
    summary = summarize_distribution(trailing["pct_above"])
    histogram = build_histogram_table(trailing["pct_above"])

    return trailing, target_row, summary, histogram, missing


def format_report(
    trailing: pd.DataFrame,
    target_row: pd.Series,
    summary: dict[str, float | int],
    histogram: pd.DataFrame,
    universe_size: int,
    missing: list[str],
    lookback: int,
) -> str:
    """Format a plain-text report for CLI output."""
    lines = [
        "NASDAQ-100 Breadth Report",
        f"Universe size: {universe_size}",
        f"Window: {trailing['trade_date'].iloc[0].date()} to {trailing['trade_date'].iloc[-1].date()} ({len(trailing)} sessions)",
        f"Signal: close > {lookback}-day SMA",
    ]

    if missing:
        lines.append(f"Missing parquet symbols: {len(missing)} ({', '.join(missing)})")
    else:
        lines.append("Missing parquet symbols: 0")

    lines.extend(
        [
            "",
            f"As of {target_row['trade_date'].date()}",
            f"Above {lookback}-day SMA: {int(target_row['above_count'])} ({target_row['pct_above']:.2f}%)",
            f"At or below {lookback}-day SMA: {int(target_row['below_or_equal_count'])} ({target_row['pct_below_or_equal']:.2f}%)",
            f"Unavailable: {int(target_row['unavailable_count'])}",
            "",
            "Trailing distribution for daily % above 5-day SMA",
            f"Mean: {summary['mean']:.2f}%",
            f"Median: {summary['median']:.2f}%",
            f"Std dev: {summary['std']:.2f} pts",
            f"Min / Max: {summary['min']:.2f}% / {summary['max']:.2f}%",
            f"P05 / P10 / P25: {summary['p05']:.2f}% / {summary['p10']:.2f}% / {summary['p25']:.2f}%",
            f"P75 / P90 / P95: {summary['p75']:.2f}% / {summary['p90']:.2f}% / {summary['p95']:.2f}%",
            "",
            "Breadth histogram",
            histogram.to_string(index=False),
        ]
    )
    return "\n".join(lines)


def main() -> None:  # pragma: no cover
    parser = argparse.ArgumentParser(description="NASDAQ-100 5-day SMA breadth analysis")
    parser.add_argument("--preset", type=Path, default=DEFAULT_PRESET, help="Universe preset JSON")
    parser.add_argument("--warehouse", type=Path, default=WAREHOUSE, help="Warehouse root path")
    parser.add_argument("--end-date", type=str, default="2026-03-11", help="Inclusive analysis end date (YYYY-MM-DD)")
    parser.add_argument("--sessions", type=int, default=252, help="Trailing trading sessions to summarize")
    parser.add_argument("--lookback", type=int, default=5, help="Simple moving average lookback")
    parser.add_argument("--csv-out", type=Path, default=None, help="Optional CSV path for trailing daily breadth")
    parser.add_argument("--json-out", type=Path, default=None, help="Optional JSON path for summary metrics")
    args = parser.parse_args()

    trailing, target_row, summary, histogram, missing = analyze_breadth(
        preset_path=args.preset,
        warehouse=args.warehouse,
        end_date=args.end_date,
        sessions=args.sessions,
        lookback=args.lookback,
    )

    report = format_report(
        trailing=trailing,
        target_row=target_row,
        summary=summary,
        histogram=histogram,
        universe_size=len(load_universe(args.preset)),
        missing=missing,
        lookback=args.lookback,
    )
    print(report)

    if args.csv_out:
        args.csv_out.parent.mkdir(parents=True, exist_ok=True)
        trailing.to_csv(args.csv_out, index=False)

    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(json.dumps(summary, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
