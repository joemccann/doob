/// Symbol discovery from the bronze parquet layer.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::config::bronze_equity_dir;

/// Return all symbols currently stored in the canonical bronze layer.
///
/// Scans for `symbol=<TICKER>` directories containing `data.parquet`.
pub fn discover_symbols(bronze_dir: Option<&Path>) -> Result<Vec<String>> {
    let root = match bronze_dir {
        Some(p) => p.to_path_buf(),
        None => bronze_equity_dir(None)?,
    };

    if !root.exists() {
        bail!("Bronze equity directory not found: {}", root.display());
    }

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type()?.is_dir() && name.starts_with("symbol=") {
            let ticker = name.splitn(2, '=').nth(1).unwrap_or("").to_string();
            if entry.path().join("data.parquet").exists() {
                entries.push((ticker, entry.path()));
            }
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries.into_iter().map(|(ticker, _)| ticker).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_discover_symbols() {
        let tmp = tempfile::tempdir().unwrap();
        let bronze = tmp.path();

        // Create symbol directories
        let spy_dir = bronze.join("symbol=SPY");
        fs::create_dir_all(&spy_dir).unwrap();
        fs::write(spy_dir.join("data.parquet"), b"fake").unwrap();

        let aapl_dir = bronze.join("symbol=AAPL");
        fs::create_dir_all(&aapl_dir).unwrap();
        fs::write(aapl_dir.join("data.parquet"), b"fake").unwrap();

        // Dir without parquet — should be skipped
        let no_data = bronze.join("symbol=NOPE");
        fs::create_dir_all(&no_data).unwrap();

        // Non-symbol dir — should be skipped
        let other = bronze.join("other_dir");
        fs::create_dir_all(&other).unwrap();

        let symbols = discover_symbols(Some(bronze)).unwrap();
        assert_eq!(symbols, vec!["AAPL", "SPY"]);
    }

    #[test]
    fn test_discover_nonexistent_dir() {
        let result = discover_symbols(Some(Path::new("/tmp/nonexistent-bronze-test")));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_bronze_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // Empty directory, no symbol=* subdirs
        let symbols = discover_symbols(Some(tmp.path())).unwrap();
        assert!(symbols.is_empty());
    }

    #[test]
    fn test_skips_dirs_without_parquet() {
        let tmp = tempfile::tempdir().unwrap();
        let bronze = tmp.path();

        // Dir with parquet
        let spy_dir = bronze.join("symbol=SPY");
        fs::create_dir_all(&spy_dir).unwrap();
        fs::write(spy_dir.join("data.parquet"), b"fake").unwrap();

        // Dir without parquet — should be skipped
        let empty_dir = bronze.join("symbol=EMPTY");
        fs::create_dir_all(&empty_dir).unwrap();

        let symbols = discover_symbols(Some(bronze)).unwrap();
        assert_eq!(symbols, vec!["SPY"]);
    }
}
