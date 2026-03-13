"""Unified CLI entrypoint for the doob backtesting package.

Usage:
    python -m doob run overnight-drift [OPTIONS]
    python -m doob run intraday-drift [OPTIONS]
    python -m doob run breadth-washout [OPTIONS]
    python -m doob run ndx100-sma-breadth [OPTIONS]
    python -m doob list-strategies
    python -m doob list-presets
"""

from __future__ import annotations

import sys


STRATEGY_MAP = {
    "overnight-drift": "doob.strategies.overnight_drift",
    "intraday-drift": "doob.strategies.intraday_drift",
    "breadth-washout": "doob.strategies.breadth_washout",
    "ndx100-sma-breadth": "doob.strategies.ndx100_sma_breadth",
    "ndx100-breadth-washout": "doob.strategies.ndx100_breadth_washout",
}


def list_strategies() -> None:
    """Print available strategy names."""
    print("Available strategies:")
    for name in sorted(STRATEGY_MAP):
        print(f"  {name}")


def list_presets() -> None:
    """Print available preset names."""
    from doob.data.presets import list_presets as _list_presets

    presets = _list_presets()
    if not presets:
        print("No presets found.")
        return
    print("Available presets:")
    for name in presets:
        print(f"  {name}")


def run_strategy(strategy_name: str) -> None:
    """Import and run a strategy's main() function."""
    if strategy_name not in STRATEGY_MAP:
        print(f"Unknown strategy: {strategy_name}")
        print(f"Available: {', '.join(sorted(STRATEGY_MAP))}")
        sys.exit(1)

    import importlib

    module = importlib.import_module(STRATEGY_MAP[strategy_name])
    module.main()


def main() -> None:
    if len(sys.argv) < 2:
        print("Usage: python -m doob <command> [args]")
        print("Commands: run, list-strategies, list-presets")
        sys.exit(1)

    command = sys.argv[1]

    if command == "list-strategies":
        list_strategies()
    elif command == "list-presets":
        list_presets()
    elif command == "run":
        if len(sys.argv) < 3:
            print("Usage: python -m doob run <strategy-name> [strategy-args...]")
            print(f"Strategies: {', '.join(sorted(STRATEGY_MAP))}")
            sys.exit(1)
        strategy_name = sys.argv[2]
        # Remove 'run' and strategy name from argv so strategy argparse works
        sys.argv = [sys.argv[0]] + sys.argv[3:]
        run_strategy(strategy_name)
    else:
        print(f"Unknown command: {command}")
        print("Commands: run, list-strategies, list-presets")
        sys.exit(1)
