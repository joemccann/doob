/// Shared strategy utilities: buy-and-hold, report formatting, daily returns.

use chrono::NaiveDate;
use num_format::{Locale, ToFormattedString};

use crate::metrics::fees::ibkr_roundtrip_cost;
use crate::metrics::performance::{annual_returns_table, cagr, max_drawdown, sharpe_default, var_95};

/// Compute daily returns from an equity curve.
pub fn daily_returns(equity: &[f64]) -> Vec<f64> {
    equity
        .windows(2)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect()
}

/// Simulate buy-and-hold equity curve.
pub fn buy_and_hold_equity(closes: &[f64], capital: f64) -> Vec<f64> {
    let n = closes.len();
    let shares = (capital / closes[0]) as i64;
    let cost = ibkr_roundtrip_cost(capital, closes[0]);
    let cash_remainder = capital - shares as f64 * closes[0];

    let mut equity = Vec::with_capacity(n + 1);
    equity.push(capital);
    for i in 0..n {
        equity.push(shares as f64 * closes[i] + cash_remainder - cost);
    }
    equity
}

/// Strategy result with equity curve and name.
pub struct StrategyResult {
    pub name: String,
    pub equity: Vec<f64>,
}

/// Format the standard results table header.
pub fn format_results_header() -> (String, String) {
    let header = format!(
        "{:<25} {:>14} {:>8} {:>8} {:>8} {:>8}",
        "Strategy", "Final ($)", "CAGR", "Sharpe", "MaxDD", "VaR95"
    );
    let sep = "-".repeat(header.len());
    (header, sep)
}

/// Format a single strategy row in the results table.
pub fn format_strategy_row(name: &str, equity: &[f64], years: f64) -> String {
    let dr = daily_returns(equity);
    let c = cagr(equity, years);
    let s = sharpe_default(&dr);
    let md = max_drawdown(equity);
    let v = var_95(&dr);

    let final_val = *equity.last().unwrap() as i64;
    let final_str = final_val.to_formatted_string(&Locale::en);

    format!(
        "{:<25} {:>14} {:>7.1}% {:>8.2} {:>7.1}% {:>8.4}",
        name,
        final_str,
        c * 100.0,
        s,
        md * 100.0,
        v
    )
}

/// Format annual returns table for multiple strategies.
pub fn format_annual_table(
    strategies: &[StrategyResult],
    dates: &[NaiveDate],
    start_year: i32,
    col_width: usize,
) -> String {
    let mut lines = Vec::new();

    // Header
    let mut header = format!("{:<6}", "Year");
    for strat in strategies {
        header += &format!(" {:>width$}", strat.name, width = col_width);
    }
    lines.push(header.clone());
    lines.push("-".repeat(header.len()));

    // Compute tables
    let tables: Vec<Vec<(i32, f64)>> = strategies
        .iter()
        .map(|s| annual_returns_table(&s.equity, dates, start_year))
        .collect();

    // Collect all years
    let mut all_years: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for tbl in &tables {
        for (year, _) in tbl {
            all_years.insert(*year);
        }
    }

    for year in all_years {
        let mut row = format!("{:<6}", year);
        for tbl in &tables {
            if let Some((_, ret)) = tbl.iter().find(|(y, _)| *y == year) {
                row += &format!(" {:>width$.1}%", ret * 100.0, width = col_width - 1);
            } else {
                row += &format!(" {:>width$}", "N/A", width = col_width);
            }
        }
        lines.push(row);
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daily_returns() {
        let equity = [100.0, 110.0, 105.0];
        let dr = daily_returns(&equity);
        assert_eq!(dr.len(), 2);
        assert!((dr[0] - 0.1).abs() < 1e-10);
        assert!((dr[1] - (-5.0 / 110.0)).abs() < 1e-10);
    }

    #[test]
    fn test_buy_and_hold() {
        let closes = [100.0, 110.0, 105.0];
        let equity = buy_and_hold_equity(&closes, 10_000.0);
        assert_eq!(equity.len(), 4);
        assert_eq!(equity[0], 10_000.0);
        // equity[1] = shares * closes[0] + cash_remainder - cost (purchase day)
        // equity[2] = shares * closes[1] + cash_remainder - cost
        let shares = 100i64;
        let cost = ibkr_roundtrip_cost(10_000.0, 100.0);
        let expected_day2 = shares as f64 * 110.0 + (10_000.0 - shares as f64 * 100.0) - cost;
        assert!(
            (equity[2] - expected_day2).abs() < 1e-6,
            "equity[2]={}, expected={}, cost={}",
            equity[2], expected_day2, cost
        );
    }

    #[test]
    fn test_format_strategy_row_produces_output() {
        let equity = [1_000_000.0, 1_100_000.0, 1_050_000.0, 1_200_000.0];
        let row = format_strategy_row("Test Strategy", &equity, 1.0);
        assert!(row.contains("Test Strategy"));
        assert!(row.contains("1,200,000"));
    }
}
