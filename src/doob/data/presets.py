"""Preset loading and validation."""

from __future__ import annotations

import json
from pathlib import Path

from doob.config import presets_dir


def load_preset(name_or_path: str | Path) -> tuple[str, list[str]]:
    """Load a preset by name (looks in presets/) or by path.

    Returns ``(preset_name, tickers)``.
    """
    path = Path(name_or_path)
    if not path.suffix:
        path = presets_dir() / f"{name_or_path}.json"
    if not path.exists():
        raise FileNotFoundError(f"Preset not found: {path}")

    payload = json.loads(path.read_text())
    name = str(payload.get("name") or path.stem)
    tickers = payload.get("tickers")
    if not isinstance(tickers, list) or not tickers:
        raise ValueError(f"Preset {path} does not contain a non-empty ticker list")

    seen: set[str] = set()
    unique: list[str] = []
    for ticker in tickers:
        t = str(ticker).upper()
        if t not in seen:
            seen.add(t)
            unique.append(t)

    return name, unique


def list_presets() -> list[str]:
    """Discover available preset names in the presets directory."""
    root = presets_dir()
    if not root.exists():
        return []
    return sorted(p.stem for p in root.glob("*.json"))
