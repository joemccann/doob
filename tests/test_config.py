"""Tests for doob.config."""

from __future__ import annotations

from pathlib import Path

import pytest

from doob import config as config_mod


class TestWarehouseRoot:
    def test_default_path(self, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
        warehouse = tmp_path / "market-warehouse"
        (warehouse / "data-lake" / "bronze").mkdir(parents=True)
        monkeypatch.setattr(config_mod, "_DEFAULT_WAREHOUSE", warehouse)
        monkeypatch.delenv("DOOB_WAREHOUSE_PATH", raising=False)

        assert config_mod.warehouse_root() == warehouse

    def test_env_var_overrides_default(self, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
        warehouse = tmp_path / "custom"
        (warehouse / "data-lake" / "bronze").mkdir(parents=True)
        monkeypatch.setenv("DOOB_WAREHOUSE_PATH", str(warehouse))

        assert config_mod.warehouse_root() == warehouse

    def test_missing_warehouse_raises(self, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
        monkeypatch.setattr(config_mod, "_DEFAULT_WAREHOUSE", tmp_path / "nonexistent")
        monkeypatch.delenv("DOOB_WAREHOUSE_PATH", raising=False)

        with pytest.raises(FileNotFoundError, match="Warehouse not found"):
            config_mod.warehouse_root()

    def test_missing_bronze_raises(self, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
        warehouse = tmp_path / "bad-warehouse"
        warehouse.mkdir()
        monkeypatch.setattr(config_mod, "_DEFAULT_WAREHOUSE", warehouse)
        monkeypatch.delenv("DOOB_WAREHOUSE_PATH", raising=False)

        with pytest.raises(FileNotFoundError, match="data-lake/bronze"):
            config_mod.warehouse_root()

    def test_dotenv_fallback(self, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
        warehouse = tmp_path / "env-warehouse"
        (warehouse / "data-lake" / "bronze").mkdir(parents=True)
        env_file = tmp_path / ".env"
        env_file.write_text(f"DOOB_WAREHOUSE_PATH={warehouse}\n")
        monkeypatch.setattr(config_mod, "_PROJECT_ROOT", tmp_path)
        monkeypatch.setattr(config_mod, "_DEFAULT_WAREHOUSE", tmp_path / "nonexistent")
        monkeypatch.delenv("DOOB_WAREHOUSE_PATH", raising=False)

        assert config_mod.warehouse_root() == warehouse


class TestBronzeEquityDir:
    def test_returns_correct_path(self, tmp_path: Path):
        warehouse = tmp_path / "wh"
        (warehouse / "data-lake" / "bronze" / "asset_class=equity").mkdir(parents=True)
        result = config_mod.bronze_equity_dir(warehouse=warehouse)
        assert result == warehouse / "data-lake" / "bronze" / "asset_class=equity"


class TestOutputAndPresetsDir:
    def test_output_dir_is_path(self):
        result = config_mod.output_dir()
        assert isinstance(result, Path)
        assert result.name == "output"

    def test_presets_dir_is_path(self):
        result = config_mod.presets_dir()
        assert isinstance(result, Path)
        assert result.name == "presets"
