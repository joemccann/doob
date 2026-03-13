"""Tests for the doob CLI."""

from __future__ import annotations

import subprocess
import sys
from unittest.mock import MagicMock

import pytest

from doob.cli import list_presets, list_strategies, main, run_strategy


class TestListStrategies:
    def test_prints_strategies(self, capsys):
        list_strategies()
        output = capsys.readouterr().out
        assert "overnight-drift" in output
        assert "intraday-drift" in output
        assert "breadth-washout" in output


class TestListPresets:
    def test_prints_presets(self, capsys):
        list_presets()
        output = capsys.readouterr().out
        assert "ndx100" in output

    def test_no_presets(self, capsys, monkeypatch):
        monkeypatch.setattr(
            "doob.data.presets.list_presets", lambda: []
        )
        list_presets()
        output = capsys.readouterr().out
        assert "No presets found" in output


class TestRunStrategy:
    def test_unknown_strategy_exits(self):
        with pytest.raises(SystemExit):
            run_strategy("nonexistent-strategy")

    def test_runs_strategy(self, monkeypatch):
        mock_module = MagicMock()
        monkeypatch.setattr(
            "importlib.import_module",
            lambda name: mock_module,
        )
        run_strategy("overnight-drift")
        mock_module.main.assert_called_once()


class TestMain:
    def test_no_args_exits(self, monkeypatch):
        monkeypatch.setattr(sys, "argv", ["doob"])
        with pytest.raises(SystemExit):
            main()

    def test_list_strategies_command(self, monkeypatch, capsys):
        monkeypatch.setattr(sys, "argv", ["doob", "list-strategies"])
        main()
        output = capsys.readouterr().out
        assert "overnight-drift" in output

    def test_list_presets_command(self, monkeypatch, capsys):
        monkeypatch.setattr(sys, "argv", ["doob", "list-presets"])
        main()
        output = capsys.readouterr().out
        assert "ndx100" in output

    def test_run_no_strategy_exits(self, monkeypatch):
        monkeypatch.setattr(sys, "argv", ["doob", "run"])
        with pytest.raises(SystemExit):
            main()

    def test_run_strategy_dispatches(self, monkeypatch):
        mock_module = MagicMock()
        monkeypatch.setattr("importlib.import_module", lambda name: mock_module)
        monkeypatch.setattr(sys, "argv", ["doob", "run", "overnight-drift", "--help"])
        main()
        mock_module.main.assert_called_once()

    def test_unknown_command_exits(self, monkeypatch):
        monkeypatch.setattr(sys, "argv", ["doob", "bogus"])
        with pytest.raises(SystemExit):
            main()


class TestCliSubprocess:
    def test_list_strategies_subprocess(self):
        result = subprocess.run(
            [sys.executable, "-m", "doob", "list-strategies"],
            capture_output=True,
            text=True,
            check=False,
            cwd="/Users/joemccann/dev/apps/finance/doob",
        )
        assert result.returncode == 0
        assert "overnight-drift" in result.stdout
