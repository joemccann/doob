"""Tests for doob.data.presets."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from doob.data import presets as presets_mod
from doob.data.presets import list_presets, load_preset


class TestLoadPreset:
    def test_load_by_path(self, tmp_path: Path):
        preset = tmp_path / "test.json"
        preset.write_text(json.dumps({"name": "test-set", "tickers": ["SPY", "QQQ"]}))

        name, tickers = load_preset(preset)
        assert name == "test-set"
        assert tickers == ["SPY", "QQQ"]

    def test_load_by_name(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
        preset = tmp_path / "sp500.json"
        preset.write_text(json.dumps({"name": "sp500", "tickers": ["AAPL", "MSFT"]}))
        monkeypatch.setattr(presets_mod, "presets_dir", lambda: tmp_path)

        name, tickers = load_preset("sp500")
        assert name == "sp500"
        assert tickers == ["AAPL", "MSFT"]

    def test_deduplicates_tickers(self, tmp_path: Path):
        preset = tmp_path / "dups.json"
        preset.write_text(json.dumps({"tickers": ["spy", "SPY", "qqq"]}))

        name, tickers = load_preset(preset)
        assert tickers == ["SPY", "QQQ"]

    def test_missing_file_raises(self, tmp_path: Path):
        with pytest.raises(FileNotFoundError):
            load_preset(tmp_path / "nonexistent.json")

    def test_empty_tickers_raises(self, tmp_path: Path):
        preset = tmp_path / "empty.json"
        preset.write_text(json.dumps({"tickers": []}))

        with pytest.raises(ValueError, match="non-empty ticker list"):
            load_preset(preset)

    def test_malformed_json_raises(self, tmp_path: Path):
        preset = tmp_path / "bad.json"
        preset.write_text("{not valid json")

        with pytest.raises(json.JSONDecodeError):
            load_preset(preset)


class TestListPresets:
    def test_lists_presets(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
        (tmp_path / "a.json").write_text("{}")
        (tmp_path / "b.json").write_text("{}")
        (tmp_path / "not-json.txt").write_text("")
        monkeypatch.setattr(presets_mod, "presets_dir", lambda: tmp_path)

        result = list_presets()
        assert result == ["a", "b"]

    def test_empty_dir(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
        monkeypatch.setattr(presets_mod, "presets_dir", lambda: tmp_path)
        assert list_presets() == []

    def test_missing_dir(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
        monkeypatch.setattr(presets_mod, "presets_dir", lambda: tmp_path / "nonexistent")
        assert list_presets() == []
