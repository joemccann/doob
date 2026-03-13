"""Generic breadth washout forward-return strategy.

Signal:
- Build a universe of symbols from either:
  - official point-in-time index membership
  - a preset file
  - an explicit ticker list
  - all currently stored symbols in the warehouse
- For each trading day, compute the share of that universe that closed above
  and at or below its simple moving average.
- Trigger either the oversold or overbought variant at the configured
  threshold.

Outputs:
- Trigger dates and breadth statistics
- Forward-return summary for configured assets over named horizons
"""

from __future__ import annotations

import argparse
import json
import re
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass, field
from pathlib import Path

import pandas as pd
import requests

from doob.config import output_dir, presets_dir
from doob.data.discovery import discover_symbols
from doob.strategies.ndx100_sma_breadth import (
    DEFAULT_FORWARD_HORIZONS,
    DEFAULT_HTTP_HEADERS,
    compute_forward_returns,
    compute_point_in_time_breadth,
    fetch_nasdaq_memberships,
    fetch_yahoo_daily_series,
    select_trailing_sessions,
)

PRESET_DIR = presets_dir()
OUTPUT_DIR = output_dir()
STRATEGY_SLUG = "breadth_washout"
DEFAULT_SIGNAL_THRESHOLDS = {
    "oversold": 65.0,
    "overbought": 70.0,
}
DEFAULT_NDX_SNAPSHOT_DATES = (
    "2025-03-11",
    "2025-05-19",
    "2025-07-25",
    "2025-07-28",
    "2025-11-07",
    "2025-11-10",
    "2025-12-22",
    "2026-01-05",
    "2026-01-16",
    "2026-01-20",
)
NAMED_UNIVERSES = {
    "ndx100": {"mode": "official-index", "label": "ndx100", "index_symbol": "NDX"},
    "sp500": {"mode": "preset", "label": "sp500", "preset_path": PRESET_DIR / "sp500.json"},
    "r2k": {"mode": "preset", "label": "r2k", "preset_path": PRESET_DIR / "r2k.json"},
    "all-stocks": {"mode": "all-stocks", "label": "all-stocks"},
}


@dataclass(frozen=True)
class BreadthWashoutConfig:
    """Configuration for the breadth washout strategy."""

    end_date: str = "2026-03-11"
    sessions: int = 252
    lookback: int = 5
    signal_mode: str = "oversold"
    signal_threshold: float = DEFAULT_SIGNAL_THRESHOLDS["oversold"]
    universe_mode: str = "official-index"
    universe_label: str = "ndx100"
    index_symbol: str | None = "NDX"
    membership_time_of_day: str = "EOD"
    membership_snapshot_dates: tuple[str, ...] = DEFAULT_NDX_SNAPSHOT_DATES
    preset_path: str | None = None
    explicit_tickers: tuple[str, ...] = ()
    bronze_dir: str | None = None
    forward_assets: tuple[str, ...] = ("SPY", "SPXL")
    horizons: dict[str, int] = field(default_factory=lambda: DEFAULT_FORWARD_HORIZONS.copy())
    adjusted_forward_returns: bool = True
    max_workers: int = 12


def slugify(value: str) -> str:
    """Return a filesystem-safe slug."""
    return re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")


def threshold_slug(value: float) -> str:
    """Return a compact filesystem-safe threshold label."""
    if float(value).is_integer():
        return f"{int(value)}pct"
    return f"{str(value).replace('.', 'p')}pct"


def normalize_symbol_for_yahoo(symbol: str) -> str:
    """Translate a canonical ticker into the Yahoo chart-symbol form."""
    return str(symbol).upper().replace(".", "-")


def signal_column(signal_mode: str) -> str:
    """Return the breadth column used to trigger the requested signal."""
    if signal_mode == "oversold":
        return "pct_below_or_equal"
    if signal_mode == "overbought":
        return "pct_above"
    raise ValueError(f"Unsupported signal mode: {signal_mode}")


def signal_summary(signal_mode: str, threshold: float, lookback: int) -> str:
    """Describe the configured breadth trigger in plain English."""
    if signal_mode == "oversold":
        return f"oversold when >= {threshold:.2f}% of universe is at/below {lookback}-day SMA"
    if signal_mode == "overbought":
        return f"overbought when >= {threshold:.2f}% of universe is above {lookback}-day SMA"
    raise ValueError(f"Unsupported signal mode: {signal_mode}")


def empty_membership_change_table() -> pd.DataFrame:
    """Return an empty membership-change table with the expected columns."""
    return pd.DataFrame(columns=["trade_date", "member_count", "added", "removed"])


def default_analysis_start(end_date: str | pd.Timestamp, sessions: int, lookback: int) -> pd.Timestamp:
    """Compute a conservative price-history start date for the strategy window."""
    end_ts = pd.Timestamp(end_date)
    return end_ts - pd.Timedelta(days=max(sessions * 2, lookback * 10))


def load_preset_metadata(path: str | Path) -> tuple[str, list[str]]:
    """Return ``(preset_name, tickers)`` from a preset JSON file."""
    payload = json.loads(Path(path).read_text())
    name = str(payload.get("name") or Path(path).stem)
    tickers = payload.get("tickers")
    if not isinstance(tickers, list) or not tickers:
        raise ValueError(f"Preset {path} does not contain a non-empty ticker list")
    return name, [str(ticker).upper() for ticker in tickers]


def discover_all_stocks(bronze_dir: str | None = None) -> list[str]:
    """Return all symbols currently stored in the canonical bronze layer."""
    return discover_symbols(bronze_dir=bronze_dir)


def fetch_price_panel(
    symbols: list[str] | tuple[str, ...],
    start_date: str | pd.Timestamp,
    end_date: str | pd.Timestamp,
    adjusted: bool,
    max_workers: int = 12,
) -> tuple[pd.DataFrame, list[str]]:
    """Fetch a close/adjusted-close panel from Yahoo, in parallel."""
    unique_symbols = sorted({str(symbol).upper() for symbol in symbols})

    def _fetch(symbol: str) -> tuple[str, pd.Series]:
        session = requests.Session()
        session.headers.update(DEFAULT_HTTP_HEADERS)
        series = fetch_yahoo_daily_series(
            normalize_symbol_for_yahoo(symbol),
            start_date=start_date,
            end_date=end_date,
            adjusted=adjusted,
            session=session,
        )
        series.name = symbol
        return symbol, series

    series_by_symbol: dict[str, pd.Series] = {}
    missing: list[str] = []
    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        futures = {executor.submit(_fetch, symbol): symbol for symbol in unique_symbols}
        for future in as_completed(futures):
            symbol, series = future.result()
            series_by_symbol[symbol] = series
            if series.empty:
                missing.append(symbol)

    frame = pd.DataFrame(series_by_symbol).sort_index()
    frame.index.name = "trade_date"
    return frame, sorted(missing)


def build_membership_change_table(memberships: dict[pd.Timestamp, set[str]]) -> pd.DataFrame:
    """Summarize official point-in-time membership changes across the window."""
    rows: list[dict[str, object]] = []
    previous: set[str] | None = None

    for trade_date in sorted(memberships):
        current = memberships[trade_date]
        if previous is not None and current != previous:
            rows.append(
                {
                    "trade_date": pd.Timestamp(trade_date),
                    "member_count": len(current),
                    "added": ",".join(sorted(current - previous)),
                    "removed": ",".join(sorted(previous - current)),
                }
            )
        previous = current

    return pd.DataFrame(rows)


def expand_snapshot_memberships(
    trade_dates: pd.DatetimeIndex | list[pd.Timestamp],
    snapshots: dict[pd.Timestamp, set[str]],
) -> dict[pd.Timestamp, set[str]]:
    """Expand dated snapshot memberships across all trade dates."""
    if not snapshots:
        raise ValueError("snapshots must be non-empty")

    ordered_snapshots = sorted((pd.Timestamp(k), v) for k, v in snapshots.items())
    expanded: dict[pd.Timestamp, set[str]] = {}
    pointer = 0

    for trade_date in pd.to_datetime(list(trade_dates)):
        while pointer + 1 < len(ordered_snapshots) and ordered_snapshots[pointer + 1][0] <= trade_date:
            pointer += 1
        expanded[pd.Timestamp(trade_date)] = ordered_snapshots[pointer][1]

    return expanded


def build_static_memberships(
    trade_dates: pd.DatetimeIndex | list[pd.Timestamp],
    symbols: list[str] | tuple[str, ...] | set[str],
) -> dict[pd.Timestamp, set[str]]:
    """Use the same ticker set for every trade date."""
    normalized = {str(symbol).upper() for symbol in symbols}
    if not normalized:
        raise ValueError("static universe symbol set must be non-empty")
    return {pd.Timestamp(trade_date): normalized for trade_date in pd.to_datetime(list(trade_dates))}


def resolve_static_universe_symbols(config: BreadthWashoutConfig) -> tuple[str, list[str]]:
    """Resolve symbols for non-index universes."""
    if config.universe_mode == "preset":
        if not config.preset_path:
            raise ValueError("preset universe mode requires preset_path")
        preset_name, tickers = load_preset_metadata(config.preset_path)
        return config.universe_label or preset_name, tickers
    if config.universe_mode == "tickers":
        tickers = [symbol.upper() for symbol in config.explicit_tickers]
        if not tickers:
            raise ValueError("tickers universe mode requires explicit_tickers")
        return config.universe_label or "tickers", tickers
    if config.universe_mode == "all-stocks":
        return config.universe_label or "all-stocks", discover_all_stocks(bronze_dir=config.bronze_dir)
    raise ValueError(f"Unsupported static universe mode: {config.universe_mode}")


def resolve_universe_memberships(  # pragma: no cover
    config: BreadthWashoutConfig,
    trade_dates: pd.DatetimeIndex,
    session: requests.Session,
) -> tuple[str, dict[pd.Timestamp, set[str]], pd.DataFrame, list[str]]:
    """Resolve the universe memberships for the strategy window."""
    if config.universe_mode == "official-index":
        if not config.index_symbol:
            raise ValueError("official-index mode requires index_symbol")
        snapshots = fetch_nasdaq_memberships(
            pd.to_datetime(config.membership_snapshot_dates),
            symbol=config.index_symbol,
            time_of_day=config.membership_time_of_day,
            session=session,
        )
        memberships = expand_snapshot_memberships(trade_dates, snapshots)
        changes = build_membership_change_table(snapshots)
        symbols = sorted({symbol for members in snapshots.values() for symbol in members})
        return config.universe_label, memberships, changes, symbols

    label, symbols = resolve_static_universe_symbols(config)
    memberships = build_static_memberships(trade_dates, symbols)
    return label, memberships, empty_membership_change_table(), sorted(symbols)


def summarize_signal_forward_returns(
    breadth: pd.DataFrame,
    asset_closes: pd.DataFrame,
    signal_mode: str,
    threshold: float,
    horizons: dict[str, int] | None = None,
) -> tuple[pd.DataFrame, pd.DataFrame]:
    """Summarize realized forward returns after the requested breadth trigger."""
    if not 0.0 <= threshold <= 100.0:
        raise ValueError("signal threshold must be between 0 and 100")

    trigger_column = signal_column(signal_mode)
    horizons = horizons or DEFAULT_FORWARD_HORIZONS
    breadth_frame = breadth.copy()
    breadth_frame["trade_date"] = pd.to_datetime(breadth_frame["trade_date"])
    triggered = breadth_frame[breadth_frame[trigger_column] >= threshold].copy().reset_index(drop=True)
    if triggered.empty:
        raise ValueError(f"No breadth observations matched {trigger_column} >= {threshold}")

    aligned_prices = asset_closes.copy()
    aligned_prices.index = pd.to_datetime(aligned_prices.index)
    aligned_prices = aligned_prices.sort_index()

    rows: list[dict[str, float | int | str]] = []
    for asset in aligned_prices.columns:
        forward = compute_forward_returns(aligned_prices[asset], horizons=horizons).reindex(triggered["trade_date"])
        for horizon_label in horizons:
            realized = forward[horizon_label].dropna()
            positive_rate = (realized > 0).mean() * 100.0 if not realized.empty else float("nan")
            rows.append(
                {
                    "asset": asset,
                    "horizon": horizon_label,
                    "signals": int(len(triggered)),
                    "observations": int(len(realized)),
                    "mean_return_pct": float(realized.mean() * 100.0) if not realized.empty else float("nan"),
                    "median_return_pct": float(realized.median() * 100.0) if not realized.empty else float("nan"),
                    "positive_rate_pct": float(positive_rate),
                }
            )

    return triggered, pd.DataFrame(rows)


def format_strategy_report(results: dict) -> str:
    """Format a plain-text report for the strategy run."""
    config: BreadthWashoutConfig = results["config"]
    target_row: pd.Series = results["target_row"]
    triggered: pd.DataFrame = results["triggered"]
    summary: pd.DataFrame = results["forward_summary"]
    changes: pd.DataFrame = results["membership_changes"]
    missing_constituent_prices: list[str] = results["missing_constituent_prices"]
    universe_label: str = results["universe_label"]

    lines = [
        f"Breadth Washout Strategy ({universe_label})",
        f"Window: {results['trailing_breadth']['trade_date'].iloc[0].date()} to {results['trailing_breadth']['trade_date'].iloc[-1].date()} ({len(results['trailing_breadth'])} sessions)",
        f"Signal: {signal_summary(config.signal_mode, config.signal_threshold, config.lookback)}",
        f"Forward returns: {'adjusted close' if config.adjusted_forward_returns else 'close'} for {', '.join(config.forward_assets)}",
        "",
        f"As of {target_row['trade_date'].date()}",
        f"Above {config.lookback}-day SMA: {int(target_row['above_count'])}",
        f"At or below {config.lookback}-day SMA: {int(target_row['below_or_equal_count'])} ({target_row['pct_below_or_equal']:.2f}%)",
        f"Signals in trailing window: {len(triggered)}",
    ]

    if missing_constituent_prices:
        lines.append(f"Missing constituent price symbols: {', '.join(missing_constituent_prices)}")
    else:
        lines.append("Missing constituent price symbols: none")

    if not changes.empty:
        lines.extend(
            [
                "",
                "Membership change dates",
                changes[["trade_date", "added", "removed"]].to_string(index=False),
            ]
        )

    lines.extend(
        [
            "",
            "Forward-return summary",
            summary.to_string(index=False),
        ]
    )
    return "\n".join(lines)


def save_strategy_outputs(results: dict, output_dir: Path = OUTPUT_DIR) -> dict[str, Path]:
    """Write the main strategy artifacts to disk."""
    config: BreadthWashoutConfig = results["config"]
    end_label = pd.Timestamp(config.end_date).date().isoformat()
    universe_slug = slugify(results["universe_label"])
    signal_slug = f"{config.signal_mode}_{threshold_slug(config.signal_threshold)}"

    output_dir.mkdir(parents=True, exist_ok=True)
    summary_path = output_dir / f"{STRATEGY_SLUG}_{universe_slug}_{signal_slug}_{end_label}_summary.csv"
    trigger_path = output_dir / f"{STRATEGY_SLUG}_{universe_slug}_{signal_slug}_{end_label}_triggers.csv"
    changes_path = output_dir / f"{STRATEGY_SLUG}_{universe_slug}_{signal_slug}_{end_label}_membership_changes.csv"
    meta_path = output_dir / f"{STRATEGY_SLUG}_{universe_slug}_{signal_slug}_{end_label}.json"

    results["forward_summary"].to_csv(summary_path, index=False)
    results["triggered"][
        ["trade_date", "pct_above", "pct_below_or_equal", "above_count", "below_or_equal_count", "eligible_count", "unavailable_count"]
    ].to_csv(trigger_path, index=False)
    results["membership_changes"].to_csv(changes_path, index=False)
    meta_path.write_text(
        json.dumps(
            {
                "config": {
                    **asdict(config),
                    "forward_assets": list(config.forward_assets),
                    "explicit_tickers": list(config.explicit_tickers),
                },
                "universe_label": results["universe_label"],
                "target_row": results["target_row"].to_dict(),
                "missing_constituent_prices": results["missing_constituent_prices"],
                "missing_forward_assets": results["missing_forward_assets"],
                "trigger_count": int(len(results["triggered"])),
            },
            indent=2,
            default=str,
        )
    )

    return {
        "summary": summary_path,
        "triggers": trigger_path,
        "membership_changes": changes_path,
        "meta": meta_path,
    }


def run_strategy(config: BreadthWashoutConfig) -> dict:  # pragma: no cover
    """Run the breadth washout strategy and return all derived artifacts."""
    analysis_start = default_analysis_start(config.end_date, config.sessions, config.lookback)

    calendar_session = requests.Session()
    calendar_session.headers.update(DEFAULT_HTTP_HEADERS)
    calendar = fetch_yahoo_daily_series(
        config.forward_assets[0],
        start_date=analysis_start,
        end_date=config.end_date,
        adjusted=False,
        session=calendar_session,
    )
    trade_dates = calendar.index
    if trade_dates.empty:
        raise ValueError("Could not determine a trading calendar from the lead forward asset")

    universe_label, memberships, membership_changes, universe_symbols = resolve_universe_memberships(
        config,
        trade_dates,
        session=calendar_session,
    )

    constituent_prices, missing_constituent_prices = fetch_price_panel(
        universe_symbols,
        start_date=analysis_start,
        end_date=config.end_date,
        adjusted=False,
        max_workers=config.max_workers,
    )
    breadth = compute_point_in_time_breadth(constituent_prices, memberships, lookback=config.lookback)
    trailing_breadth = select_trailing_sessions(breadth, end_date=config.end_date, sessions=config.sessions)
    target_row = trailing_breadth.loc[trailing_breadth["trade_date"] == pd.Timestamp(config.end_date)].iloc[0]

    forward_prices, missing_forward_assets = fetch_price_panel(
        list(config.forward_assets),
        start_date=trailing_breadth["trade_date"].min(),
        end_date=config.end_date,
        adjusted=config.adjusted_forward_returns,
        max_workers=min(config.max_workers, len(config.forward_assets)),
    )
    triggered, forward_summary = summarize_signal_forward_returns(
        trailing_breadth,
        forward_prices,
        signal_mode=config.signal_mode,
        threshold=config.signal_threshold,
        horizons=config.horizons,
    )

    return {
        "config": config,
        "analysis_start": analysis_start,
        "universe_label": universe_label,
        "memberships": memberships,
        "membership_changes": membership_changes,
        "trailing_breadth": trailing_breadth,
        "target_row": target_row,
        "triggered": triggered,
        "forward_summary": forward_summary,
        "constituent_prices": constituent_prices,
        "forward_prices": forward_prices,
        "missing_constituent_prices": missing_constituent_prices,
        "missing_forward_assets": missing_forward_assets,
    }


def parse_horizons(values: list[str] | None) -> dict[str, int]:
    """Parse CLI horizon arguments of the form ``1w=5``."""
    if not values:
        return DEFAULT_FORWARD_HORIZONS.copy()

    parsed: dict[str, int] = {}
    for value in values:
        if "=" not in value:
            raise ValueError(f"Invalid horizon '{value}'; expected label=periods")
        label, raw_periods = value.split("=", 1)
        parsed[label] = int(raw_periods)
    return parsed


def build_config_from_args(args: argparse.Namespace) -> BreadthWashoutConfig:
    """Translate parsed CLI arguments into a strategy config."""
    signal_threshold = (
        args.threshold
        if args.threshold is not None
        else (args.min_pct_below if args.signal_mode == "oversold" else DEFAULT_SIGNAL_THRESHOLDS[args.signal_mode])
    )

    if args.tickers:
        return BreadthWashoutConfig(
            end_date=args.end_date,
            sessions=args.sessions,
            lookback=args.lookback,
            signal_mode=args.signal_mode,
            signal_threshold=signal_threshold,
            universe_mode="tickers",
            universe_label=args.universe_label or "tickers",
            explicit_tickers=tuple(symbol.upper() for symbol in args.tickers),
            forward_assets=tuple(symbol.upper() for symbol in args.assets),
            horizons=parse_horizons(args.horizon),
            adjusted_forward_returns=not args.price_returns,
            max_workers=args.max_workers,
        )

    if args.preset:
        preset_name, _ = load_preset_metadata(args.preset)
        return BreadthWashoutConfig(
            end_date=args.end_date,
            sessions=args.sessions,
            lookback=args.lookback,
            signal_mode=args.signal_mode,
            signal_threshold=signal_threshold,
            universe_mode="preset",
            universe_label=args.universe_label or preset_name,
            preset_path=str(Path(args.preset)),
            forward_assets=tuple(symbol.upper() for symbol in args.assets),
            horizons=parse_horizons(args.horizon),
            adjusted_forward_returns=not args.price_returns,
            max_workers=args.max_workers,
        )

    named = NAMED_UNIVERSES[args.universe]
    return BreadthWashoutConfig(
        end_date=args.end_date,
        sessions=args.sessions,
        lookback=args.lookback,
        signal_mode=args.signal_mode,
        signal_threshold=signal_threshold,
        universe_mode=named["mode"],
        universe_label=args.universe_label or named["label"],
        index_symbol=named.get("index_symbol"),
        membership_time_of_day=args.membership_time_of_day,
        membership_snapshot_dates=tuple(args.snapshot_date) if args.snapshot_date else DEFAULT_NDX_SNAPSHOT_DATES,
        preset_path=str(named["preset_path"]) if "preset_path" in named else None,
        bronze_dir=args.bronze_dir,
        forward_assets=tuple(symbol.upper() for symbol in args.assets),
        horizons=parse_horizons(args.horizon),
        adjusted_forward_returns=not args.price_returns,
        max_workers=args.max_workers,
    )


def main() -> None:  # pragma: no cover
    parser = argparse.ArgumentParser(description="Generic breadth washout strategy")
    parser.add_argument("--end-date", type=str, default="2026-03-11", help="Signal evaluation end date (YYYY-MM-DD)")
    parser.add_argument("--sessions", type=int, default=252, help="Trailing trading sessions in the study window")
    parser.add_argument("--lookback", type=int, default=5, help="Breadth SMA lookback")
    parser.add_argument("--signal-mode", choices=sorted(DEFAULT_SIGNAL_THRESHOLDS), default="oversold", help="Breadth signal mode")
    parser.add_argument("--threshold", type=float, default=None, help="Generic trigger threshold percent; defaults to 65 for oversold and 70 for overbought")
    parser.add_argument("--min-pct-below", type=float, default=65.0, help="Backward-compatible oversold threshold alias when --signal-mode oversold")
    parser.add_argument("--universe", choices=sorted(NAMED_UNIVERSES), default="ndx100", help="Named universe")
    parser.add_argument("--preset", type=str, default=None, help="Custom preset JSON path; overrides --universe")
    parser.add_argument("--tickers", nargs="+", default=None, help="Explicit ticker list; overrides --preset and --universe")
    parser.add_argument("--universe-label", type=str, default=None, help="Optional report label for the chosen universe")
    parser.add_argument("--membership-time-of-day", type=str, default="EOD", choices=["SOD", "EOD"], help="Official index membership snapshot timing")
    parser.add_argument("--snapshot-date", action="append", default=None, help="Official membership snapshot date (YYYY-MM-DD)")
    parser.add_argument("--bronze-dir", type=str, default=None, help="Optional bronze dir override for all-stocks mode")
    parser.add_argument("--assets", nargs="+", default=["SPY", "SPXL"], help="Forward-return assets")
    parser.add_argument("--horizon", action="append", default=None, help="Forward horizon, e.g. --horizon 1w=5")
    parser.add_argument("--price-returns", action="store_true", help="Use raw close instead of adjusted close for forward returns")
    parser.add_argument("--max-workers", type=int, default=12, help="Concurrent Yahoo fetch workers")
    args = parser.parse_args()

    config = build_config_from_args(args)
    results = run_strategy(config)
    paths = save_strategy_outputs(results)
    print(format_strategy_report(results))
    print("\nFiles")
    for label, path in paths.items():
        print(f"{label}: {path}")


if __name__ == "__main__":
    main()
