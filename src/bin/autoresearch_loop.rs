use chrono::{Datelike, NaiveDate, Utc, Duration};
use clap::Parser;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use std::{
    collections::{HashSet},
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

const BREADTH_STRATEGIES: &[&str] = &[
    "breadth-washout",
    "breadth-ma",
    "breadth-dual-ma",
    "ndx100-breadth-washout",
];

const SEED_QUERIES: &[&str] = &[
    "site:arxiv.org quantitative market breadth trading strategy",
    "site:arxiv.org volatility regime switching trading strategy",
    "site:arxiv.org momentum mean reversion breakout trading",
];

const HORIZON_1W: &str = "1w=5";
const HORIZON_1M: &str = "1m=21";

const KNOWN_BREADTH_ASSETS: &[&str] = &["SPY", "QQQ", "SPXL", "IWM"];

#[derive(Clone, Debug)]
struct Candidate {
    candidate_id: String,
    strategy: String,
    args: Vec<String>,
    rationale: String,
    source: String,
    focus_assets: Vec<String>,
    target_horizon: String,
    min_observations: u32,
    min_signals: u32,
    is_diagnostic: bool,
}

#[derive(Clone, Debug)]
struct ScoredRun {
    score: f64,
    details: Value,
}

#[derive(Serialize, Clone)]
struct CandidateReport {
    candidate_id: String,
    strategy: String,
    category: String,
    args: Vec<String>,
    rationale: String,
    source: String,
    focus_assets: Vec<String>,
    target_horizon: String,
    train_score: f64,
    test_score: f64,
    combined_score: f64,
    train_details: Value,
    test_details: Value,
}

#[derive(Serialize)]
struct ExaSeed {
    query: String,
    title: String,
    url: String,
    text: String,
}

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<Value>,
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = String::from("target/release/doob"))]
    doob_bin: String,

    #[arg(long, default_value_t = 80)]
    candidates: usize,

    #[arg(long, default_value_t = 20)]
    top: usize,

    #[arg(long)]
    seed_web: bool,

    #[arg(long, default_value_t = 17)]
    random_seed: u64,

    #[arg(long, default_value = "2020-01-01")]
    train_start: String,

    #[arg(long, default_value = "2024-12-31")]
    train_end: String,

    #[arg(long, default_value = "2025-01-01")]
    test_start: String,

    #[arg(long, default_value = "2026-03-11")]
    test_end: String,

    #[arg(long, default_value_t = 1008)]
    train_sessions: i64,

    #[arg(long, default_value_t = 252)]
    test_sessions: i64,

    #[arg(long)]
    verbose: bool,
}

fn safe_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(num) => num.as_f64().filter(|v| v.is_finite()),
        Value::String(s) => s.parse::<f64>().ok().filter(|v| v.is_finite()),
        Value::Bool(true) => Some(1.0),
        Value::Bool(false) => Some(0.0),
        _ => None,
    }
}

fn safe_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(num) => num.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn extract_seed_tags(seed: &ExaSeed) -> HashSet<&'static str> {
    let blob = format!("{} {}", seed.title, seed.text);
    let blob = normalize_text(&blob);
    let mut tags = HashSet::new();

    if contains_any(&blob, &["momentum", "trend", "momentum strategy", "trend strategy", "top-down"]) {
        tags.insert("momentum");
    }
    if contains_any(&blob, &["mean reversion", "reversion", "contrarian", "reversal", "value"]) {
        tags.insert("reversion");
    }
    if contains_any(&blob, &["regime", "volatility", "vix", "risk regime"]) {
        tags.insert("regime");
    }
    if contains_any(&blob, &["intraday", "open", "close", "session", "minute", "daily"]) {
        tags.insert("intraday");
    }
    if contains_any(&blob, &["market breadth", "breadth", "sma", "cross-sectional"]) {
        tags.insert("breadth");
    }

    tags
}

fn fetch_exa_ideas(queries: &[&str], limit: usize) -> Vec<ExaSeed> {
    let api_key = env::var("EXA_API_KEY").unwrap_or_default();
    if api_key.trim().is_empty() {
        return Vec::new();
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .ok();
    let Some(client) = client else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut ideas = Vec::new();

    for query in queries {
        let payload = json!({
            "query": query,
            "numResults": limit,
            "contents": { "text": true },
        });

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );

        let response = client
            .post("https://api.exa.ai/search")
            .headers(headers)
            .json(&payload)
            .send()
            .and_then(|r| r.error_for_status());
        let Ok(response) = response else {
            continue;
        };
        let body = response.text().unwrap_or_default();
        let parsed: Result<ExaResponse, _> = serde_json::from_str(&body);
        if parsed.is_err() {
            continue;
        }
        let parsed = parsed.unwrap_or(ExaResponse { results: Vec::new() });

        for row in parsed.results {
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let url = row
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if title.is_empty() || url.is_empty() {
                continue;
            }

            let key = format!("{}||{}", title.to_lowercase(), url);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);

            let text = row
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();

            ideas.push(ExaSeed {
                query: query.to_string(),
                title,
                url,
                text,
            });
        }
    }

    ideas
}

fn add_candidate(pool: &mut Vec<Candidate>, cand: Candidate) {
    pool.push(cand);
}

fn push_horizon(args: &mut Vec<String>, horizon: &str) {
    args.push("--horizon".to_string());
    args.push(horizon.to_string());
}

fn build_seed_candidates(seed: &ExaSeed, idx: usize) -> Vec<Candidate> {
    let tags = extract_seed_tags(seed);
    let source = if seed.url.is_empty() {
        "exa".to_string()
    } else {
        seed.url.clone()
    };
    let title = if seed.title.trim().is_empty() {
        "seed paper".to_string()
    } else {
        seed.title.clone()
    };
    if title.trim().is_empty() {
        return Vec::new();
    }

    let base = &KNOWN_BREADTH_ASSETS[..3];
    let mut out = Vec::new();

    if tags.contains("momentum") || tags.contains("breadth") {
        let lookback = 10 + (idx * 3) % 20;
        let threshold = 55 + (idx * 2) % 15;
        let mut args = vec![
            "--universe".to_string(),
            "ndx100".to_string(),
            "--lookback".to_string(),
            lookback.to_string(),
            "--signal-mode".to_string(),
            "oversold".to_string(),
            "--threshold".to_string(),
            threshold.to_string(),
            "--assets".to_string(),
        ];
        args.extend(base.iter().map(|s| s.to_string()));
        push_horizon(&mut args, HORIZON_1M);
        out.push(Candidate {
            candidate_id: format!("seed-mom-{idx:02}"),
            strategy: "breadth-washout".to_string(),
            args,
            rationale: format!("Web seed-driven momentum candidate: {title}"),
            source: source.clone(),
            focus_assets: base.iter().map(|s| s.to_string()).collect(),
            target_horizon: "1m".to_string(),
            min_observations: 18,
            min_signals: 8,
            is_diagnostic: false,
        });
    }

    if tags.contains("regime") {
        let short = 8 + (idx % 3) * 2;
        let longs = [50, 100, 200];
        let long = longs[idx % longs.len()];
        let mut args = vec![
            "--short-period".to_string(),
            short.to_string(),
            "--long-period".to_string(),
            long.to_string(),
            "--threshold".to_string(),
            (15 + (idx % 3) * 5).to_string(),
            "--universe".to_string(),
            "sp500".to_string(),
            "--assets".to_string(),
        ];
        args.extend(base.iter().map(|s| s.to_string()));
        push_horizon(&mut args, HORIZON_1M);
        out.push(Candidate {
            candidate_id: format!("seed-reg-{idx:02}"),
            strategy: "breadth-dual-ma".to_string(),
            args,
            rationale: format!("Web seed-driven regime candidate: {title}"),
            source: source.clone(),
            focus_assets: base.iter().map(|s| s.to_string()).collect(),
            target_horizon: "1m".to_string(),
            min_observations: 20,
            min_signals: 8,
            is_diagnostic: false,
        });
    }

    if tags.contains("reversion") {
        let lookback = 20 + (idx % 3) * 15;
        let mut args = vec![
            "--short-period".to_string(),
            lookback.to_string(),
            "--signal-mode".to_string(),
            "oversold".to_string(),
            "--threshold".to_string(),
            (50 + (idx % 4) * 5).to_string(),
            "--universe".to_string(),
            "sp500".to_string(),
            "--assets".to_string(),
        ];
        args.extend(base.iter().map(|s| s.to_string()));
        push_horizon(&mut args, HORIZON_1W);
        out.push(Candidate {
            candidate_id: format!("seed-rev-{idx:02}"),
            strategy: "breadth-ma".to_string(),
            args,
            rationale: format!("Web seed-driven reversion candidate: {title}"),
            source: source.clone(),
            focus_assets: base.iter().map(|s| s.to_string()).collect(),
            target_horizon: "1w".to_string(),
            min_observations: 20,
            min_signals: 8,
            is_diagnostic: false,
        });
    }

    if tags.contains("intraday") {
        out.push(Candidate {
            candidate_id: format!("seed-int-{idx:02}"),
            strategy: "intraday-drift".to_string(),
            args: vec!["--ticker".to_string(), "QQQ".to_string(), "--no-plots".to_string()],
            rationale: format!("Web seed-driven intraday candidate: {title}"),
            source: source.clone(),
            focus_assets: vec!["QQQ".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 8,
            is_diagnostic: false,
        });
    }

    out
}

fn build_candidate_pool(seed_ideas: &[ExaSeed]) -> Vec<Candidate> {
    let mut pool = Vec::new();

    for universe in ["ndx100", "sp500", "r2k", "all-stocks"] {
        for lookback in [5, 10, 20] {
            for mode in ["oversold", "overbought"] {
                for threshold in [50, 60, 65, 70, 75] {
                    if universe == "all-stocks" && threshold >= 75 {
                        continue;
                    }

                    let prefix: String = universe.chars().take(4).collect();
                    let mut args = vec![
                        "--universe".to_string(),
                        universe.to_string(),
                        "--lookback".to_string(),
                        lookback.to_string(),
                        "--signal-mode".to_string(),
                        mode.to_string(),
                        "--threshold".to_string(),
                        threshold.to_string(),
                        "--assets".to_string(),
                        "SPY".to_string(),
                        "SPXL".to_string(),
                    ];
                    push_horizon(&mut args, HORIZON_1M);
                    add_candidate(
                        &mut pool,
                        Candidate {
                            candidate_id: format!("bw-{prefix}-{lookback}-{}{threshold}", &mode[..3]),
                            strategy: "breadth-washout".to_string(),
                            args,
                            rationale: "Test oversold/overbought breadth regime with short and medium MA windows."
                                .to_string(),
                            source: "grid".to_string(),
                            focus_assets: vec!["SPY".to_string(), "SPXL".to_string()],
                            target_horizon: "1m".to_string(),
                            min_observations: 16,
                            min_signals: 8,
                            is_diagnostic: false,
                        },
                    );

                    if universe == "all-stocks" {
                        break;
                    }
                }
            }
        }
    }

    for short in [2, 5, 10, 20, 50] {
        for long in [50, 100, 200] {
            if short >= long {
                continue;
            }
            for threshold in [10, 20, 30] {
                let mut args = vec![
                    "--short-period".to_string(),
                    short.to_string(),
                    "--long-period".to_string(),
                    long.to_string(),
                    "--threshold".to_string(),
                    threshold.to_string(),
                    "--universe".to_string(),
                    "ndx100".to_string(),
                    "--assets".to_string(),
                    "SPY".to_string(),
                    "SPXL".to_string(),
                ];
                push_horizon(&mut args, HORIZON_1M);
                add_candidate(
                    &mut pool,
                    Candidate {
                        candidate_id: format!("dm-{short}-{long}-t{threshold}"),
                        strategy: "breadth-dual-ma".to_string(),
                        args,
                        rationale:
                            "Pullback-in-trend structure, inspired by mean-reversion plus trend persistence literature."
                                .to_string(),
                        source: "grid".to_string(),
                        focus_assets: vec!["SPY".to_string(), "SPXL".to_string()],
                        target_horizon: "1m".to_string(),
                        min_observations: 20,
                        min_signals: 10,
                        is_diagnostic: false,
                    },
                );
            }
        }
    }

    for lookback in [8, 20, 50, 100] {
        for threshold in [50, 60, 70, 80] {
            for mode in ["oversold", "overbought"] {
                add_candidate(
                    &mut pool,
                    Candidate {
                        candidate_id: format!("bm-{lookback}-t{threshold}-{}", &mode[..3]),
                        strategy: "breadth-ma".to_string(),
                        args: vec![
                            "--short-period".to_string(),
                            lookback.to_string(),
                            "--signal-mode".to_string(),
                            mode.to_string(),
                            "--threshold".to_string(),
                            threshold.to_string(),
                            "--universe".to_string(),
                            "ndx100".to_string(),
                            "--assets".to_string(),
                            "SPY".to_string(),
                            "QQQ".to_string(),
                        ],
                        rationale:
                            "Single moving-average breadth trigger with threshold sweep and mode sweep."
                                .to_string(),
                        source: "grid".to_string(),
                        focus_assets: vec!["SPY".to_string(), "QQQ".to_string()],
                        target_horizon: "1w".to_string(),
                        min_observations: 20,
                        min_signals: 8,
                        is_diagnostic: false,
                    },
                );
            }
        }
    }

    for threshold in [55, 60, 65, 70] {
        add_candidate(
            &mut pool,
            {
                let mut args = vec![
                    "--signal-mode".to_string(),
                    if threshold % 2 == 0 {
                        "overbought".to_string()
                    } else {
                        "oversold".to_string()
                    },
                    "--threshold".to_string(),
                    threshold.to_string(),
                    "--assets".to_string(),
                    "SPY".to_string(),
                    "QQQ".to_string(),
                ];
                push_horizon(&mut args, HORIZON_1W);

                Candidate {
                    candidate_id: format!("ndx-wrap-{threshold}"),
                    strategy: "ndx100-breadth-washout".to_string(),
                    args,
                    rationale:
                        "ndx100 wrapper candidate for alternative regime gate and universe settings."
                            .to_string(),
                    source: "grid".to_string(),
                    focus_assets: vec!["SPY".to_string(), "QQQ".to_string()],
                    target_horizon: "1w".to_string(),
                    min_observations: 16,
                    min_signals: 6,
                    is_diagnostic: false,
                }
            },
        );
    }

    pool.extend([
        Candidate {
            candidate_id: "overnight-no-vix".to_string(),
            strategy: "overnight-drift".to_string(),
            args: vec!["--no-vix-filter".to_string(), "--no-plots".to_string()],
            rationale: "Execution baseline: overnight carry with no VIX gating.".to_string(),
            source: "baseline".to_string(),
            focus_assets: vec!["SPY".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
        },
        Candidate {
            candidate_id: "overnight-vix".to_string(),
            strategy: "overnight-drift".to_string(),
            args: vec!["--no-plots".to_string()],
            rationale: "Execution baseline: built-in VIX filter should prefer safer regimes.".to_string(),
            source: "baseline".to_string(),
            focus_assets: vec!["SPY".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
        },
        Candidate {
            candidate_id: "intra-long".to_string(),
            strategy: "intraday-drift".to_string(),
            args: vec!["--ticker".to_string(), "SPY".to_string(), "--no-plots".to_string()],
            rationale: "Intraday open-to-close long baseline.".to_string(),
            source: "baseline".to_string(),
            focus_assets: vec!["SPY".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
        },
        Candidate {
            candidate_id: "intra-short".to_string(),
            strategy: "intraday-drift".to_string(),
            args: vec![
                "--ticker".to_string(),
                "SPY".to_string(),
                "--short".to_string(),
                "--no-plots".to_string(),
            ],
            rationale:
                "Intraday long/short asymmetry probe on same execution structure.".to_string(),
            source: "baseline".to_string(),
            focus_assets: vec!["SPY".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
        },
    ]);

    for (i, seed) in seed_ideas.iter().take(20).enumerate() {
        pool.extend(build_seed_candidates(seed, i + 1));
    }

    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for cand in pool {
        let key = format!(
            "{}|{}|{}",
            cand.strategy,
            cand.args.join("|"),
            cand.source
        );
        if seen.insert(key) {
            unique.push(cand);
        }
    }

    unique
}

fn score_breadth(candidate: &Candidate, payload: &Value) -> Option<ScoredRun> {
    let rows = payload.get("forward_summary")?.as_array()?;
    if rows.is_empty() {
        return None;
    }

    let focus: HashSet<String> = candidate
        .focus_assets
        .iter()
        .map(|s| s.to_ascii_uppercase())
        .collect();

    let horizon_rows: Vec<&Value> = rows
        .iter()
        .filter(|row| row.get("horizon").and_then(Value::as_str).unwrap_or("") == candidate.target_horizon)
        .collect();
    let rows_for_scoring: Vec<&Value> = if horizon_rows.is_empty() {
        rows.iter().collect()
    } else {
        horizon_rows
    };

    let mut scored = Vec::new();
    for row in rows_for_scoring {
        let row_asset = row
            .get("asset")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase();
        if !focus.is_empty() && !focus.contains(&row_asset) {
            continue;
        }

        let cum = safe_float(row.get("cumulative_return_pct").unwrap_or(&Value::Null));
        let sharpe = safe_float(row.get("sharpe").unwrap_or(&Value::Null));
        let dd = safe_float(row.get("max_drawdown_pct").unwrap_or(&Value::Null));
        let var95 = safe_float(row.get("var_95_pct").unwrap_or(&Value::Null));
        let obs = safe_u64(row.get("observations").unwrap_or(&Value::Null)).unwrap_or(0);
        let sig = safe_u64(row.get("signals").unwrap_or(&Value::Null)).unwrap_or(0);

        if cum.is_none() || sharpe.is_none() || dd.is_none() || var95.is_none() {
            continue;
        }
        if obs < candidate.min_observations as u64 || sig < candidate.min_signals as u64 {
            continue;
        }

        let cum = cum.unwrap_or_default();
        let sharpe = sharpe.unwrap_or_default();
        let dd = dd.unwrap_or_default();
        let var95 = var95.unwrap_or_default();
        let pos = safe_float(row.get("positive_rate_pct").unwrap_or(&Value::Null)).unwrap_or(0.0);

        let mut score = 2.0 * (cum / 100.0);
        score += 1.4 * sharpe;
        score -= 1.2 * dd.abs() / 100.0;
        score -= 0.25 * var95.max(0.0) / 100.0;
        score += 0.002 * pos;

        scored.push((score, row.clone()));
    }

    if scored.is_empty() {
        return None;
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let (_score, best_row) = scored[0].clone();

    Some(ScoredRun {
        score: _score,
        details: json!({
            "asset": best_row.get("asset"),
            "horizon": best_row.get("horizon"),
            "observations": best_row.get("observations"),
            "signals": best_row.get("signals"),
            "cumulative_return_pct": best_row.get("cumulative_return_pct"),
            "sharpe": best_row.get("sharpe"),
            "max_drawdown_pct": best_row.get("max_drawdown_pct"),
            "var_95_pct": best_row.get("var_95_pct"),
            "profit_factor": best_row.get("profit_factor"),
        }),
    })
}

fn select_drift_row(candidate: &Candidate, payload: &Value) -> Option<Value> {
    let results = payload.get("results")?.as_array()?;
    if results.is_empty() {
        return None;
    }

    let target_name = if candidate.strategy == "overnight-drift" {
        ["Overnight (VIX Filter)", "Overnight (All)"]
            .iter()
            .find_map(|name| {
                results.iter().find_map(|row| {
                    let n = row.get("name").and_then(Value::as_str)?;
                    if n == *name {
                        Some(n.to_string())
                    } else {
                        None
                    }
                })
            })
    } else {
        results.iter().find_map(|row| {
            let n = row.get("name").and_then(Value::as_str)?;
            if n != "Buy & Hold" {
                Some(n.to_string())
            } else {
                None
            }
        })
    };

    let target_name = target_name.or_else(|| {
        results.iter().find_map(|row| {
            let n = row.get("name").and_then(Value::as_str)?;
            if n != "Buy & Hold" {
                Some(n.to_string())
            } else {
                None
            }
        })
    })?;

    results.iter().find_map(|row| {
        let n = row.get("name").and_then(Value::as_str)?;
        if n == target_name {
            Some(row.clone())
        } else {
            None
        }
    })
}

fn score_drift(candidate: &Candidate, payload: &Value) -> Option<ScoredRun> {
    let row = select_drift_row(candidate, payload)?;
    let cagr = safe_float(row.get("cagr").unwrap_or(&Value::Null))?;
    let sharpe = safe_float(row.get("sharpe").unwrap_or(&Value::Null))?;
    let dd = safe_float(row.get("max_drawdown").unwrap_or(&Value::Null))?;
    let var95 = safe_float(row.get("var_95").unwrap_or(&Value::Null))?;

    let score = 2.0 * cagr + 1.2 * sharpe - 1.8 * dd.abs() - 0.5 * var95.max(0.0);

    Some(ScoredRun {
        score,
        details: json!({
            "name": row.get("name"),
            "final_equity": row.get("final_equity"),
            "cagr": row.get("cagr"),
            "sharpe": row.get("sharpe"),
            "max_drawdown": row.get("max_drawdown"),
            "var_95": row.get("var_95"),
        }),
    })
}

fn score_payload(candidate: &Candidate, payload: &Value) -> Option<ScoredRun> {
    if BREADTH_STRATEGIES
        .iter()
        .any(|s| *s == candidate.strategy.as_str())
    {
        return score_breadth(candidate, payload);
    }
    if candidate.strategy == "overnight-drift" || candidate.strategy == "intraday-drift" {
        return score_drift(candidate, payload);
    }
    None
}

fn has_arg(args: &[String], flag: &str) -> bool {
    args.iter().any(|v| v == flag)
}

fn strategy_category(strategy: &str) -> &'static str {
    match strategy {
        "breadth-washout" => "Breadth",
        "breadth-ma" => "Breadth",
        "breadth-dual-ma" => "Breadth",
        "ndx100-breadth-washout" => "Breadth",
        "overnight-drift" => "Drift",
        "intraday-drift" => "Drift",
        _ => "Other",
    }
}

fn asset_summary(assets: &[String], max_chars: usize) -> String {
    let joined = assets.join(", ");
    if joined.len() <= max_chars {
        return joined;
    }
    let truncated = &joined[..max_chars.saturating_sub(3)];
    format!("{}...", truncated)
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= max_chars {
        return normalized;
    }
    if max_chars <= 3 {
        return normalized[..max_chars].to_string();
    }
    format!("{}...", &normalized[..max_chars - 3])
}

fn format_command_line(cmd: &Path, args: &[String]) -> String {
    let mut parts = vec![cmd.display().to_string()];
    for arg in args {
        if arg.contains(' ') {
            parts.push(format!("\"{}\"", arg));
        } else {
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

fn format_detail_summary(details: &Value) -> String {
    let asset = details
        .get("asset")
        .and_then(Value::as_str)
        .or_else(|| details.get("name").and_then(Value::as_str))
        .unwrap_or("?");
    let horizon = details.get("horizon").and_then(Value::as_str).unwrap_or("-");
    let obs = details.get("observations").and_then(Value::as_u64).unwrap_or(0);
    let signals = details.get("signals").and_then(Value::as_u64).unwrap_or(0);
    let cum = safe_float(details.get("cumulative_return_pct").or_else(|| details.get("cagr")).unwrap_or(&Value::Null)).unwrap_or(0.0);
    let sharpe = safe_float(details.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let dd = safe_float(
        details
            .get("max_drawdown_pct")
            .or_else(|| details.get("max_drawdown"))
            .unwrap_or(&Value::Null),
    )
    .unwrap_or(0.0);
    let var95 = safe_float(details
        .get("var_95_pct")
        .or_else(|| details.get("var_95"))
        .unwrap_or(&Value::Null),
    )
    .unwrap_or(0.0);
    format!(
        "asset={asset} horizon={horizon} obs={obs} signals={signals} cum={cum:.2} sharpe={sharpe:.3} dd={dd:.2} var95={var95:.3}"
    )
}

fn format_args_summary(args: &[String], max_len: usize) -> String {
    let inline = if args.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", args.join(", "))
    };

    if inline.len() <= max_len {
        return inline;
    }

    let mut shortened = String::from("[");
    for (idx, arg) in args.iter().enumerate() {
        if idx > 0 {
            shortened.push_str(", ");
        }
        if shortened.len() + arg.len() + 4 >= max_len {
            shortened.push_str("...");
            break;
        }
        shortened.push_str(arg);
    }
    shortened.push(']');
    shortened
}

fn run_candidate(
    candidate: &Candidate,
    doob_bin: &Path,
    end_date: &str,
    start_date: Option<&str>,
    sessions: Option<i64>,
    stage: &str,
    verbose: bool,
) -> Option<ScoredRun> {
    let mut args: Vec<String> = vec![
        "--output".to_string(),
        "json".to_string(),
        "run".to_string(),
        candidate.strategy.clone(),
    ];
    args.extend(candidate.args.iter().cloned());

    if BREADTH_STRATEGIES
        .iter()
        .any(|s| *s == candidate.strategy.as_str())
    {
        args.push("--end-date".to_string());
        args.push(end_date.to_string());
        if let Some(sessions) = sessions {
            if !has_arg(&candidate.args, "--sessions") {
                args.push("--sessions".to_string());
                args.push(sessions.to_string());
            }
        }
    } else if candidate.strategy == "overnight-drift" || candidate.strategy == "intraday-drift" {
        if let Some(start_date) = start_date {
            args.push("--start-date".to_string());
            args.push(start_date.to_string());
        }
        args.push("--end-date".to_string());
        args.push(end_date.to_string());
    } else {
        args.push("--end-date".to_string());
        args.push(end_date.to_string());
    }

    if verbose {
        println!("  [{stage}] {}", format_command_line(doob_bin, &args));
    }

    let mut cmd = Command::new(doob_bin);
    cmd.args(&args);

    let output = cmd.output().ok()?;
    if !output.status.success() {
        if verbose {
            eprintln!(
                "  doob failed (code={:?})",
                output.status.code().unwrap_or(-1)
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                eprintln!("  stderr: {}", stderr.trim());
            }
        }
        return None;
    }

    let payload = match serde_json::from_slice(&output.stdout) {
        Ok(payload) => payload,
        Err(err) => {
            if verbose {
                eprintln!("  failed parsing doob output: {err}");
                let stdout = String::from_utf8_lossy(&output.stdout);
                eprintln!("  stdout: {}", stdout.trim());
            }
            return None;
        }
    };
    score_payload(candidate, &payload)
}

fn estimate_sessions(start_date: &str, end_date: &str) -> Option<i64> {
    let start = NaiveDate::parse_from_str(start_date, "%Y-%m-%d").ok()?;
    let end = NaiveDate::parse_from_str(end_date, "%Y-%m-%d").ok()?;
    if end < start {
        return None;
    }

    let mut day = start;
    let mut count = 0i64;
    while day <= end {
        if day.weekday().num_days_from_monday() < 5 {
            count += 1;
        }
        day += Duration::days(1);
    }
    Some(count)
}

fn sessions_for_window(start_date: &str, end_date: &str, fallback: i64) -> i64 {
    estimate_sessions(start_date, end_date).unwrap_or(fallback).max(1)
}

fn evaluate_candidate(
    candidate: &Candidate,
    idx: usize,
    total: usize,
    doob_bin: &Path,
    train_start: &str,
    train_end: &str,
    test_start: &str,
    test_end: &str,
    train_sessions: i64,
    test_sessions: i64,
    verbose: bool,
) -> Option<CandidateReport> {
    if candidate.is_diagnostic {
        return None;
    }

    let train_window_sessions = sessions_for_window(train_start, train_end, train_sessions);
    let test_window_sessions = sessions_for_window(test_start, test_end, test_sessions);

    if verbose {
        println!(
            "\n{} {}",
            format!("┏ Candidate {}/{}", idx, total),
            candidate.candidate_id
        );
        println!(
            "┣ strategy: {} | category: {} | horizon: {}",
            candidate.strategy,
            strategy_category(&candidate.strategy),
            candidate.target_horizon
        );
        println!("┣ assets: {}", asset_summary(&candidate.focus_assets, 80));
        println!("┣ source: {}", candidate.source);
        println!("┣ rationale: {}", truncate_text(&candidate.rationale, 120));
        println!("┣ args: {}", format_args_summary(&candidate.args, 220));
        println!("┣ train window: {train_start} -> {train_end} (sessions={train_window_sessions})");
    }
    let train_run =
        run_candidate(
            candidate,
            doob_bin,
            train_end,
            Some(train_start),
            Some(train_window_sessions),
            "train",
            verbose,
        )?;
    if verbose {
        println!("  train score: {:.4}", train_run.score);
        println!("  train summary: {}", format_detail_summary(&train_run.details));
        println!("  test window: {test_start} -> {test_end} (sessions={test_window_sessions})");
    }
    let test_run =
        run_candidate(
            candidate,
            doob_bin,
            test_end,
            Some(test_start),
            Some(test_window_sessions),
            "test",
            verbose,
        )?;
    if verbose {
        println!("  test score: {:.4}", test_run.score);
        println!("  test summary: {}", format_detail_summary(&test_run.details));
    }

    let combined_score = 0.65 * train_run.score + 0.35 * test_run.score;

    Some(CandidateReport {
        candidate_id: candidate.candidate_id.clone(),
        strategy: candidate.strategy.clone(),
        category: strategy_category(&candidate.strategy).to_string(),
        args: candidate.args.clone(),
        rationale: candidate.rationale.clone(),
        source: candidate.source.clone(),
        focus_assets: candidate.focus_assets.clone(),
        target_horizon: candidate.target_horizon.clone(),
        train_score: train_run.score,
        test_score: test_run.score,
        combined_score,
        train_details: train_run.details,
        test_details: test_run.details,
    })
}

fn shuffle_candidates(items: &mut [Candidate], seed: u64) {
    if items.len() < 2 {
        return;
    }

    let mut state = seed.wrapping_add(0x9E3779B97F4A7C15);
    for i in (1..items.len()).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

fn save_ledger(path: &Path, rows: &[CandidateReport]) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = File::options().append(true).create(true).open(path)?;
    let ts = Utc::now().to_rfc3339();

    for row in rows {
        let line = json!({
            "timestamp": ts,
            "candidate_id": row.candidate_id.clone(),
            "strategy": row.strategy.clone(),
            "category": row.category.clone(),
            "args": row.args.clone(),
            "rationale": row.rationale.clone(),
            "source": row.source.clone(),
            "focus_assets": row.focus_assets.clone(),
            "target_horizon": row.target_horizon.clone(),
            "train_score": row.train_score,
            "test_score": row.test_score,
            "combined_score": row.combined_score,
            "train_details": row.train_details.clone(),
            "test_details": row.test_details.clone(),
        });
        writeln!(f, "{}", serde_json::to_string(&line).unwrap_or_else(|_| "{}".to_string()))?;
    }
    Ok(())
}

fn print_top(rows: &[CandidateReport], k: usize, verbose: bool) {
    let mut rows = rows.to_vec();
    rows.sort_by(|a, b| b.combined_score.partial_cmp(&a.combined_score).unwrap_or(std::cmp::Ordering::Equal));

    println!("\nTop candidates:");
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "rank",
        "score",
        "category",
        "strategy",
        "id/assets",
        "source",
        "train",
        "test",
        "horizon",
    ]);

    for (i, row) in rows.iter().take(k).enumerate() {
        let id_assets = if row.focus_assets.is_empty() {
            row.candidate_id.clone()
        } else {
            format!(
                "{} [{}]",
                row.candidate_id,
                asset_summary(&row.focus_assets, 18)
            )
        };
        table.add_row(vec![
            (i + 1).to_string(),
            format!("{:.3}", row.combined_score),
            row.category.clone(),
            row.strategy.clone(),
            id_assets,
            row.source.clone(),
            format!("{:.3}", row.train_score),
            format!("{:.3}", row.test_score),
            row.target_horizon.clone(),
        ]);
    }
    println!("{}", table);
    if verbose {
        println!("Top candidates displayed as ranked table. Use --top to control row count.");
    }
}

fn print_best(best: &CandidateReport) {
    println!("\nBest candidate:");
    let mut best_table = Table::new();
    best_table.load_preset(UTF8_FULL);
    best_table.set_content_arrangement(ContentArrangement::Dynamic);
    best_table.set_header(vec!["field", "value"]);
    best_table.add_row(vec!["strategy".to_string(), best.strategy.clone()]);
    best_table.add_row(vec!["candidate_id".to_string(), best.candidate_id.clone()]);
    best_table.add_row(vec!["category".to_string(), best.category.clone()]);
    best_table.add_row(vec![
        "assets".to_string(),
        asset_summary(&best.focus_assets, 140),
    ]);
    best_table.add_row(vec!["horizon".to_string(), best.target_horizon.clone()]);
    best_table.add_row(vec!["source".to_string(), best.source.clone()]);
    best_table.add_row(vec![
        "score".to_string(),
        format!("{:.4}", best.combined_score),
    ]);
    best_table.add_row(vec![
        "train/test".to_string(),
        format!("{:.4} / {:.4}", best.train_score, best.test_score),
    ]);
    best_table.add_row(vec![
        "command".to_string(),
        format!(
            "doob --output json run {} {}",
            best.strategy,
            format_args_summary(&best.args, 240)
        ),
    ]);
    best_table.add_row(vec!["rationale".to_string(), truncate_text(&best.rationale, 220)]);
    best_table.add_row(vec!["args".to_string(), format_args_summary(&best.args, 260)]);
    best_table.add_row(vec![
        "train details".to_string(),
        format_detail_summary(&best.train_details),
    ]);
    best_table.add_row(vec![
        "test details".to_string(),
        format_detail_summary(&best.test_details),
    ]);
    println!("{}", best_table);
}

fn save_seed_ideas(path: &Path, items: &[ExaSeed]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = json!({
        "generated_at": Utc::now().to_rfc3339(),
        "count": items.len(),
        "items": items,
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, text)
}

fn main() {
    let args = Args::parse();
    let doob_bin = PathBuf::from(&args.doob_bin);
    if !doob_bin.exists() {
        eprintln!("doob binary not found: {}", doob_bin.display());
        return;
    }

    let mut seed_ideas = Vec::new();
    println!(
        "Autoresearch run: strategy seed web: {} | evaluating up to {} candidates",
        args.seed_web, args.candidates
    );
    println!(
        "Data windows: train {} -> {} (fallback sessions {}) | test {} -> {} (fallback sessions {})",
        args.train_start,
        args.train_end,
        args.train_sessions,
        args.test_start,
        args.test_end,
        args.test_sessions
    );
    if args.verbose {
        println!(
            "autoresearch settings: candidates={} top={} train_sessions={} test_sessions={}",
            args.candidates, args.top, args.train_sessions, args.test_sessions
        );
        println!(
            "train window: {} -> {} | test window: {} -> {}",
            args.train_start, args.train_end, args.test_start, args.test_end
        );
    }
    if args.seed_web {
        seed_ideas = fetch_exa_ideas(SEED_QUERIES, 6);
        if args.verbose {
            println!("fetched {} web seeds", seed_ideas.len());
        }
        let report_path = Path::new("reports/autoresearch-exa-ideas.json");
        if let Err(err) = save_seed_ideas(report_path, &seed_ideas) {
            eprintln!("Failed to write exa ideas file: {err}");
        }
    }

    let mut candidates = build_candidate_pool(&seed_ideas);
    if args.verbose {
        println!("candidate pool: {} total", candidates.len());
    }
    shuffle_candidates(&mut candidates, args.random_seed);
    if args.verbose {
        println!("candidate pool shuffled with seed {}", args.random_seed);
    }

    let mut results = Vec::new();
    for (idx, candidate) in candidates.iter().take(args.candidates).enumerate() {
        if args.verbose {
            println!("evaluating candidate {} of {}", idx + 1, args.candidates);
        }
        if let Some(report) = evaluate_candidate(
            candidate,
            idx + 1,
            args.candidates,
            &doob_bin,
            &args.train_start,
            &args.train_end,
            &args.test_start,
            &args.test_end,
            args.train_sessions,
            args.test_sessions,
            args.verbose,
        ) {
            results.push(report);
        } else if args.verbose {
            println!("  rejected: scoring gates or execution failure");
        }
    }

    if args.verbose {
        println!("completed loop: {} candidates passed", results.len());
    }
    print_top(&results, args.top, args.verbose);
    if let Err(err) = save_ledger(
        Path::new("reports/autoresearch-ledger.jsonl"),
        &results,
    ) {
        eprintln!("Failed to write ledger: {err}");
    }

    if results.is_empty() {
        println!("No candidate passed scoring gates.");
        return;
    }

    let mut ranked = results.clone();
    ranked.sort_by(|a, b| b.combined_score.partial_cmp(&a.combined_score).unwrap_or(std::cmp::Ordering::Equal));
    let best = &ranked[0];
    print_best(best);
}
