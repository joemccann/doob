/// Paper-driven trading strategy runner.
///
/// This strategy family is used by the autoresearch loop for web-sourced
/// hypotheses that are not yet encoded as native strategy families.
/// It applies a single-rule signal on close-price history and evaluates a
/// long-only equity curve against a buy-and-hold baseline.
use anyhow::{Result, bail};
use chrono::NaiveDate;

use num_format::ToFormattedString;

use crate::cli::OutputFormat;
use crate::data::readers::load_ticker_ohlcv;
use crate::metrics::fees::ibkr_roundtrip_cost;
use crate::strategies::common::{
    JsonOutput, StrategyResult, build_json_annual_returns, buy_and_hold_equity,
    compute_strategy_metrics, format_annual_table, format_results_header, format_strategy_row,
};

const DEFAULT_CAPITAL: f64 = 1_000_000.0;
const DEFAULT_TTLYRS: f64 = 365.25;
const DEFAULT_END_DATE: &str = "2026-03-11";
const DEFAULT_SESSIONS: usize = 252;
const DEFAULT_FAST_WINDOW: usize = 12;
const DEFAULT_SLOW_WINDOW: usize = 40;
const DEFAULT_RSI_WINDOW: usize = 14;
const DEFAULT_VOL_WINDOW: usize = 20;

#[derive(Debug, Clone, Copy)]
enum ResearchRule {
    TrendMomentum,
    TrendPullback,
    RsiReversion,
    VolatilityRegime,
}

impl ResearchRule {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "trend_momentum" => Some(Self::TrendMomentum),
            "trend_pullback" => Some(Self::TrendPullback),
            "rsi_reversion" => Some(Self::RsiReversion),
            "volatility_regime" => Some(Self::VolatilityRegime),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::TrendMomentum => "trend_momentum",
            Self::TrendPullback => "trend_pullback",
            Self::RsiReversion => "rsi_reversion",
            Self::VolatilityRegime => "volatility_regime",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct PaperResearchArgs {
    #[arg(long, default_value = DEFAULT_END_DATE, help = "Inclusive end date for scoring windows (YYYY-MM-DD)")]
    pub end_date: String,

    #[arg(long, default_value_t = DEFAULT_SESSIONS, help = "Trailing trading sessions")]
    pub sessions: usize,

    #[arg(
        long,
        default_value = "SPY",
        help = "Asset to generate signals and trade"
    )]
    pub asset: String,

    #[arg(
        long,
        default_value = "trend_momentum",
        help = "Research rule: trend_momentum|trend_pullback|rsi_reversion|volatility_regime"
    )]
    pub rule: String,

    #[arg(long, default_value_t = DEFAULT_FAST_WINDOW, help = "Primary lookback for MA and momentum signal rules")]
    pub fast_window: usize,

    #[arg(long, default_value_t = DEFAULT_SLOW_WINDOW, help = "Secondary MA lookback for trend rules")]
    pub slow_window: usize,

    #[arg(long, default_value_t = DEFAULT_RSI_WINDOW, help = "RSI lookback")]
    pub rsi_window: usize,

    #[arg(
        long,
        default_value_t = 35.0,
        help = "RSI oversold threshold for reversion candidates"
    )]
    pub rsi_oversold: f64,

    #[arg(
        long,
        default_value_t = 65.0,
        help = "RSI overbought threshold (not used unless strategy variant requires it)"
    )]
    pub rsi_overbought: f64,

    #[arg(long, default_value_t = DEFAULT_VOL_WINDOW, help = "Realized-volatility lookback")]
    pub vol_window: usize,

    #[arg(
        long,
        default_value_t = 0.45,
        help = "Volatility percentile cap [0,1] for volatility_regime"
    )]
    pub vol_cap: f64,

    #[arg(long, help = "Optional hypothesis id")]
    pub hypothesis_id: Option<String>,

    #[arg(long, default_value_t = 12, help = "Concurrent fetch workers")]
    pub max_workers: usize,

    #[arg(long, default_value_t = 2015, help = "Annual table start year")]
    pub start_year_table: i32,
}

fn moving_average(values: &[f64], end: usize, window: usize) -> Option<f64> {
    if window == 0 || end + 1 < window {
        return None;
    }
    let start = end + 1 - window;
    let mut sum = 0.0;
    for v in &values[start..=end] {
        if !v.is_finite() {
            return None;
        }
        sum += *v;
    }
    Some(sum / window as f64)
}

fn rolling_volatility(returns: &[f64], end: usize, window: usize) -> Option<f64> {
    if window < 2 || end + 1 < window {
        return None;
    }
    let start = end + 1 - window;
    let mut vals = Vec::new();
    for r in &returns[start..=end] {
        if !r.is_finite() {
            continue;
        }
        vals.push(*r);
    }
    if vals.len() < window {
        return None;
    }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (vals.len() as f64 - 1.0);
    let vol = var.sqrt() * DEFAULT_TTLYRS.sqrt();
    if vol.is_finite() { Some(vol) } else { None }
}

fn rolling_rsi(closes: &[f64], end: usize, window: usize) -> Option<f64> {
    if window < 2 || end < window {
        return None;
    }
    let start = end - window + 1;
    let mut gains = 0.0;
    let mut losses = 0.0;
    for idx in start..=end {
        let prev = closes[idx.saturating_sub(1)];
        let cur = closes[idx];
        if !cur.is_finite() || !prev.is_finite() {
            return None;
        }
        let d = cur - prev;
        if d > 0.0 {
            gains += d;
        } else {
            losses += -d;
        }
    }
    let avg_gain = gains / window as f64;
    let avg_loss = losses / window as f64;
    let rsi = if avg_loss <= 0.0 {
        100.0
    } else {
        let rs = avg_gain / avg_loss;
        100.0 - 100.0 / (1.0 + rs)
    };
    Some(rsi)
}

fn build_signal_mask(
    closes: &[f64],
    rule: ResearchRule,
    fast: usize,
    slow: usize,
    rsi_window: usize,
    rsi_oversold: f64,
    vol_window: usize,
    vol_cap: f64,
) -> Vec<bool> {
    if closes.len() < slow.saturating_add(4) {
        return vec![false; closes.len()];
    }

    let returns: Vec<f64> = closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect();

    let vols: Vec<f64> = closes
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            rolling_volatility(&returns, idx.saturating_sub(1), vol_window).unwrap_or(f64::NAN)
        })
        .collect();
    let valid_vol: Vec<f64> = vols.iter().copied().filter(|v| v.is_finite()).collect();
    let mut vol_threshold = None;
    if valid_vol.len() >= 5 && (0.0..=1.0).contains(&vol_cap) {
        let mut quantile_values = valid_vol.clone();
        quantile_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((quantile_values.len() as f64 - 1.0) * vol_cap).round() as usize;
        vol_threshold = quantile_values.get(idx).copied();
    }

    (0..closes.len())
        .map(|idx| {
            let signal = match rule {
                ResearchRule::TrendMomentum => {
                    let short = moving_average(closes, idx, fast);
                    let long = moving_average(closes, idx, slow);
                    match (short, long) {
                        (Some(s), Some(l)) => s > l && closes[idx] > s,
                        _ => false,
                    }
                }
                ResearchRule::TrendPullback => {
                    let short = moving_average(closes, idx, fast);
                    let long = moving_average(closes, idx, slow);
                    match (short, long) {
                        (Some(s), Some(l)) => closes[idx] < s && closes[idx] > l,
                        _ => false,
                    }
                }
                ResearchRule::RsiReversion => rolling_rsi(closes, idx, rsi_window)
                    .map(|v| v < rsi_oversold)
                    .unwrap_or(false),
                ResearchRule::VolatilityRegime => {
                    let vol = vols.get(idx).copied().unwrap_or(f64::NAN);
                    if !vol.is_finite() {
                        return false;
                    }
                    if vol_threshold.is_none() {
                        false
                    } else {
                        vol <= vol_threshold.unwrap_or(f64::INFINITY)
                    }
                }
            };
            signal && closes[idx].is_finite()
        })
        .collect()
}

fn simulate_strategy(closes: &[f64], mask: &[bool], capital: f64) -> Vec<f64> {
    let n = closes.len();
    if n < 2 {
        return vec![capital];
    }
    let mut equity = Vec::with_capacity(n + 1);
    equity.push(capital);
    let mut current = capital;
    for i in 0..n {
        if i + 1 < n {
            if mask[i] {
                let shares = (current / closes[i]) as i64;
                if shares > 0 {
                    let fee = ibkr_roundtrip_cost(current, closes[i]);
                    let pnl = shares as f64 * (closes[i + 1] - closes[i]);
                    current = current + pnl - fee;
                }
            }
        }
        equity.push(current);
    }
    equity
}

/// Run a paper-derived research strategy.
pub fn run(args: &PaperResearchArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let quiet = json_mode || fmt == OutputFormat::Md;
    let asset = args.asset.to_uppercase();
    let rule = ResearchRule::from_str(&args.rule).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported rule '{}'; expected trend_momentum, trend_pullback, rsi_reversion, volatility_regime",
            args.rule
        )
    })?;

    if !quiet {
        println!("Loading {asset} from bronze parquet...");
    }
    let mut bars = load_ticker_ohlcv(&asset, None, None)?;
    let end_date = args
        .end_date
        .parse::<NaiveDate>()
        .map_err(|e| anyhow::anyhow!("invalid --end-date: {e}"))?;
    let max_lookback = args
        .slow_window
        .max(args.vol_window)
        .max(args.rsi_window.max(2));
    let buffer = args
        .sessions
        .saturating_mul(6)
        .max(max_lookback.saturating_mul(3))
        .saturating_add(30) as i64;
    let start_date = end_date - chrono::Duration::days(buffer);

    bars.retain(|r| r.trade_date >= start_date && r.trade_date <= end_date);
    if bars.len() < args.sessions.max(40) {
        bail!(
            "insufficient data from {start_date} to {end_date} for {asset}; available {} rows",
            bars.len()
        );
    }
    bars.sort_by_key(|r| r.trade_date);

    let dates: Vec<NaiveDate> = bars.iter().map(|r| r.trade_date).collect();
    let closes: Vec<f64> = bars.iter().map(|r| r.close).collect();
    if closes.len() < 2 {
        bail!("not enough observations after filtering for {asset}");
    }

    let masks = build_signal_mask(
        &closes,
        rule,
        args.fast_window,
        args.slow_window,
        args.rsi_window,
        args.rsi_oversold,
        args.vol_window,
        args.vol_cap,
    );
    let start_idx = closes.len().saturating_sub(args.sessions);
    if start_idx == closes.len() {
        bail!(
            "insufficient rows for requested sessions {} (available {})",
            args.sessions,
            closes.len()
        );
    }
    let eval_dates = &dates[start_idx..];
    let eval_closes = &closes[start_idx..];
    let eval_mask = &masks[start_idx..];

    let mut strategies = Vec::new();
    strategies.push(StrategyResult {
        name: "Buy & Hold".to_string(),
        equity: buy_and_hold_equity(eval_closes, DEFAULT_CAPITAL),
    });

    let eq_signal = simulate_strategy(eval_closes, eval_mask, DEFAULT_CAPITAL);
    strategies.push(StrategyResult {
        name: format!("PaperResearch [{}|{}]", asset, rule.as_str()),
        equity: eq_signal,
    });

    let years = (eval_dates
        .last()
        .ok_or_else(|| anyhow::anyhow!("missing data range"))?
        .signed_duration_since(*eval_dates.first().unwrap()))
    .num_days() as f64
        / DEFAULT_TTLYRS;

    if json_mode {
        let results: Vec<_> = strategies
            .iter()
            .map(|s| compute_strategy_metrics(&s.name, &s.equity, years))
            .collect();
        let output = JsonOutput {
            strategy: "paper-research".to_string(),
            ticker: asset.clone(),
            period_start: eval_dates.first().unwrap().to_string(),
            period_end: eval_dates.last().unwrap().to_string(),
            years,
            capital: DEFAULT_CAPITAL,
            fee_model: "IBKR Tiered".to_string(),
            results,
            annual_returns: build_json_annual_returns(
                &strategies,
                eval_dates,
                args.start_year_table,
            ),
        };
        println!("{}", serde_json::to_string(&output)?);
    } else if fmt == OutputFormat::Md {
        println!(
            "{}",
            crate::strategies::common::format_results_md(
                &format!("Paper Research: {} {}", asset, rule.as_str()),
                &asset,
                eval_dates,
                years,
                DEFAULT_CAPITAL,
                &strategies,
                &[],
                args.start_year_table,
            )
        );
    } else {
        println!();
        println!("{}", "=".repeat(80));
        println!("Paper Research Strategy: {} ({})", asset, rule.as_str());
        println!(
            "Period: {} to {} ({:.1} years)",
            eval_dates.first().unwrap(),
            eval_dates.last().unwrap(),
            years
        );
        println!(
            "Capital: ${}",
            (DEFAULT_CAPITAL as i64).to_formatted_string(&num_format::Locale::en)
        );
        println!("{}", "=".repeat(80));

        let (header, sep) = format_results_header();
        println!("{header}");
        println!("{sep}");
        for strategy in &strategies {
            println!(
                "{}",
                format_strategy_row(&strategy.name, &strategy.equity, years)
            );
        }
        println!("\nAnnual Returns (from {}):", args.start_year_table);
        println!(
            "{}",
            format_annual_table(&strategies, eval_dates, args.start_year_table, 25)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moving_average_window() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        assert!((moving_average(&values, 1, 2).unwrap() - 1.5).abs() < 1e-12);
        assert!((moving_average(&values, 3, 3).unwrap() - 3.0).abs() < 1e-12);
    }

    #[test]
    fn test_rsi_bounds() {
        let closes = vec![100.0, 102.0, 101.0, 104.0, 103.0, 105.0];
        let rsi = rolling_rsi(&closes, 4, 4).unwrap_or(-1.0);
        assert!((0.0..=100.0).contains(&rsi));
    }

    #[test]
    fn test_signal_mask_modes() {
        let closes = vec![10.0, 10.5, 10.8, 10.2, 10.4, 10.9, 11.0, 11.2];
        let mask = build_signal_mask(&closes, ResearchRule::TrendMomentum, 3, 5, 3, 30.0, 3, 0.5);
        assert_eq!(mask.len(), closes.len());
    }
}
