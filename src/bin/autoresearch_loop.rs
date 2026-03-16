use chrono::{Datelike, NaiveDate, Duration, Utc};
use clap::Parser;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use dotenvy::dotenv;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashSet},
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

const RESEARCH_STRATEGY: &str = "paper-research";
const RULE_TREND_MOMENTUM: &str = "trend_momentum";
const RULE_TREND_PULLBACK: &str = "trend_pullback";
const RULE_RSI_REVERSION: &str = "rsi_reversion";
const RULE_VOL_REGIME: &str = "volatility_regime";

const SEED_QUERIES: &[&str] = &[
    "site:arxiv.org quant trading strategy momentum equities",
    "site:arxiv.org regime switching trading strategy",
    "site:arxiv.org machine learning strategy for stock prediction",
    "site:arxiv.org volatility-aware trading signal strategy",
    "site:arxiv.org intraday stock trading strategy",
    "site:arxiv.org statistical arbitrage paper equities",
];

const MIN_CANDIDATES_TARGET_DEFAULT: usize = 100;
const DEFAULT_TRAIN_SESSIONS: i64 = 1008;
const DEFAULT_TEST_SESSIONS: i64 = 252;
const FAST_WINDOW_SET: &[u32] = &[6, 8, 10, 12, 14, 16, 20, 24, 30, 35];
const SLOW_WINDOW_SET: &[u32] = &[18, 25, 35, 50, 70, 90, 120, 160, 220];
const RSI_WINDOW_SET: &[u32] = &[8, 10, 12, 14, 16, 20, 24];
const VOL_WINDOW_SET: &[u32] = &[10, 14, 18, 20, 24, 30, 35, 40];
const RSI_OVERSOLD_SET: &[u32] = &[18, 20, 22, 24, 26, 28, 30, 32, 35];
const RSI_OVERBOUGHT_SET: &[u32] = &[60, 65, 68, 70, 72, 74, 76, 78, 80];
const VOL_CAP_SET: &[f64] = &[0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.60, 0.70, 0.80];

#[derive(Clone, Debug)]
struct Candidate {
    candidate_id: String,
    strategy: String,
    rule: String,
    args: Vec<String>,
    rationale: String,
    source: String,
    focus_asset: String,
    is_seeded: bool,
    _min_signals: u32,
    _min_observations: u32,
}

#[derive(Clone, Debug)]
struct ScoredRun {
    score: f64,
    details: Value,
}

#[derive(Clone, Serialize)]
struct CandidateReport {
    candidate_id: String,
    strategy: String,
    category: String,
    args: Vec<String>,
    rule: String,
    rationale: String,
    source: String,
    focus_asset: String,
    train_score: f64,
    test_score: f64,
    combined_score: f64,
    train_details: Value,
    test_details: Value,
    is_seeded: bool,
}

#[derive(Deserialize, Serialize)]
struct ExaSeed {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaSeed>,
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = String::from("target/release/doob"))]
    doob_bin: String,

    #[arg(long, default_value_t = MIN_CANDIDATES_TARGET_DEFAULT)]
    candidates: usize,

    #[arg(long, default_value_t = 10)]
    top: usize,

    #[arg(long)]
    seed_web: bool,

    #[arg(long)]
    include_grid: bool,

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

    #[arg(long, default_value_t = DEFAULT_TRAIN_SESSIONS)]
    train_sessions: i64,

    #[arg(long, default_value_t = DEFAULT_TEST_SESSIONS)]
    test_sessions: i64,

    #[arg(long)]
    verbose: bool,
}

fn safe_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(num) => num.as_f64().filter(|v| v.is_finite()),
        Value::String(s) => s.parse::<f64>().ok().filter(|v| v.is_finite()),
        _ => None,
    }
}

fn arg_value<'a>(args: &'a [String], flag: &'a str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(v) = iter.next() {
        if v == flag {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn arg_u32(args: &[String], flag: &str) -> Option<u32> {
    arg_value(args, flag).and_then(|value: &str| value.parse::<u32>().ok())
}

fn arg_f64(args: &[String], flag: &str) -> Option<f64> {
    arg_value(args, flag).and_then(|value: &str| value.parse::<f64>().ok())
}

fn fmt_pct(v: f64) -> String {
    format!("{:.1}%", v.abs() * 100.0)
}

fn fmt_num(v: f64) -> String {
    format!("{:.3}", v)
}

fn rule_description(rule: &str, args: &[String], asset: &str) -> String {
    match rule {
        RULE_TREND_MOMENTUM => {
            let fast = arg_u32(args, "--fast-window").unwrap_or(12);
            let slow = arg_u32(args, "--slow-window").unwrap_or(40);
            format!(
                "Trend momentum on {asset}: buys when the {fast}-day moving average is above the {slow}-day average and price is above the short MA, aiming to ride persistent up-trends while exiting on MA crosses."
            )
        }
        RULE_TREND_PULLBACK => {
            let fast = arg_u32(args, "--fast-window").unwrap_or(12);
            let slow = arg_u32(args, "--slow-window").unwrap_or(40);
            format!(
                "Trend pullback on {asset}: buys when price drops below the {fast}-day MA but stays above the {slow}-day MA, capturing controlled dip re-entries inside a larger uptrend."
            )
        }
        RULE_RSI_REVERSION => {
            let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
            let oversold = arg_f64(args, "--rsi-oversold").unwrap_or(35.0);
            format!(
                "RSI reversion on {asset}: enters positions when RSI({rsi_window}) falls below {oversold:.0}, targeting mean-reversion from oversold conditions."
            )
        }
        RULE_VOL_REGIME => {
            let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
            let vol_cap = arg_f64(args, "--vol-cap").unwrap_or(0.45);
            format!(
                "Volatility regime on {asset}: holds through calmer market states, only trading when realized {vol_window}-day volatility stays within the lowest {:.0}% regime (vol_cap).",
                (vol_cap * 100.0).round()
            )
        }
        _ => format!("Paper-research candidate on {asset} with adaptive research-rule logic."),
    }
}

fn has_arg(args: &[String], flag: &str) -> bool {
    args.iter().any(|v| v == flag)
}

fn format_command_line(cmd: &Path, args: &[String]) -> String {
    let mut parts = vec![cmd.display().to_string()];
    for arg in args {
        if arg.contains(' ') {
            parts.push(format!("\"{arg}\""));
        } else {
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

fn normalize_text(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn extract_seed_tags(seed: &ExaSeed) -> HashSet<&'static str> {
    let blob = normalize_text(&format!("{} {}", seed.title, seed.text));
    let mut tags = HashSet::new();

    if contains_any(
        &blob,
        &[
            "momentum",
            "trend",
            "breakout",
            "sma",
            "moving average",
            "reversal",
        ],
    ) {
        tags.insert("momentum");
    }
    if contains_any(
        &blob,
        &[
            "volatility",
            "regime",
            "risk",
            "drawdown",
            "variance",
            "vix",
            "stress",
        ],
    ) {
        tags.insert("regime");
    }
    if contains_any(
        &blob,
        &[
            "reversion",
            "mean reversion",
            "oversold",
            "rsi",
            "oscillator",
            "mean-reverting",
        ],
    ) {
        tags.insert("reversion");
    }
    if contains_any(
        &blob,
        &[
            "intraday",
            "open",
            "close",
            "minute",
            "session",
            "high frequency",
            "hourly",
        ],
    ) {
        tags.insert("intraday");
    }

    if tags.is_empty() {
        tags.insert("momentum");
        tags.insert("reversion");
        tags.insert("regime");
    }
    tags
}

fn signature_for_candidate(candidate: &Candidate) -> String {
    format!(
        "{}|{}|{}|{}",
        candidate.strategy,
        candidate.rule,
        candidate.focus_asset,
        candidate.args.join("|")
    )
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
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );

        let payload = json!({
            "query": query,
            "numResults": limit,
            "contents": {"text": true},
        });

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
            if row.title.trim().is_empty() || row.url.trim().is_empty() {
                continue;
            }
            let key = format!("{}||{}", row.title.to_lowercase(), row.url.to_lowercase());
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            ideas.push(row);
        }
    }

    ideas
}

fn sample_or_default<T: Clone>(values: &[T], idx: usize, offset: usize) -> T {
    values[(idx + offset) % values.len()].clone()
}

fn seed_candidate(
    seed_idx: usize,
    seed: &ExaSeed,
    rule: &str,
    focus_asset: &str,
    fast: u32,
    slow: u32,
    rsi_window: Option<u32>,
    rsi_oversold: Option<u32>,
    rsi_overbought: Option<u32>,
    vol_window: Option<u32>,
    vol_cap: Option<f64>,
    variant: usize,
) -> Candidate {
    let mut args = vec![
        "--asset".to_string(),
        focus_asset.to_string(),
        "--rule".to_string(),
        rule.to_string(),
        "--fast-window".to_string(),
        fast.to_string(),
        "--slow-window".to_string(),
        slow.to_string(),
        "--rsi-window".to_string(),
        rsi_window.unwrap_or(14).to_string(),
        "--rsi-oversold".to_string(),
        rsi_oversold.unwrap_or(30).to_string(),
        "--rsi-overbought".to_string(),
        rsi_overbought.unwrap_or(70).to_string(),
        "--vol-window".to_string(),
        vol_window.unwrap_or(20).to_string(),
        "--vol-cap".to_string(),
        format!("{:.2}", vol_cap.unwrap_or(0.40)),
    ];
    if !has_arg(&args, "--hypothesis-id") {
        args.push("--hypothesis-id".to_string());
        args.push(format!("seed-{}-{}", seed_idx, variant));
    }

    Candidate {
        candidate_id: format!("seed-{seed_idx:03}-{rule}-v{variant}"),
        strategy: RESEARCH_STRATEGY.to_string(),
        rule: rule.to_string(),
        args,
        rationale: format!(
            "ArXiv-seeded hypothesis ({}): {}",
            seed_idx,
            normalize_text(&seed.title).trim()
        ),
        source: if seed.url.is_empty() {
            "seed".to_string()
        } else {
            seed.url.clone()
        },
        focus_asset: focus_asset.to_string(),
        is_seeded: true,
        _min_observations: 20,
        _min_signals: 10,
    }
}

fn build_seed_candidates(seed: &ExaSeed, idx: usize) -> Vec<Candidate> {
    let tags = extract_seed_tags(seed);
    let assets = ["SPY", "QQQ", "SPXL", "IWM", "TQQQ"];
    let mut out = Vec::new();

    let include_momentum = tags.contains("momentum") || tags.contains("intraday");
    let include_reversion = tags.contains("reversion");
    let include_regime = tags.contains("regime");

    let momentum_rules = if include_momentum {
        vec![RULE_TREND_MOMENTUM, RULE_TREND_PULLBACK]
    } else {
        vec![]
    };
    let reversion_rules = if include_reversion { vec![RULE_RSI_REVERSION] } else { vec![] };
    let regime_rules = if include_regime {
        vec![RULE_VOL_REGIME]
    } else {
        vec![]
    };
    let all_rules: Vec<&str> = momentum_rules
        .into_iter()
        .chain(reversion_rules)
        .chain(regime_rules)
        .collect();

    let selected_rules = if all_rules.is_empty() {
        vec![RULE_TREND_MOMENTUM, RULE_RSI_REVERSION, RULE_VOL_REGIME]
    } else {
        all_rules
    };

    for (offset, rule) in selected_rules.iter().enumerate() {
        for variant in 0..3 {
            let focus_asset = sample_or_default(&assets, idx, offset * 2 + variant).to_string();

            match *rule {
                RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => {
                    let fast = sample_or_default(&FAST_WINDOW_SET, idx, offset + variant) as u32;
                    let slow = sample_or_default(
                        &SLOW_WINDOW_SET,
                        idx.saturating_add(variant + 1),
                        offset + 1,
                    ) as u32;
                    if slow <= fast {
                        continue;
                    }
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        fast,
                        slow,
                        None,
                        None,
                        None,
                        None,
                        None,
                        variant,
                    ));
                }
                RULE_RSI_REVERSION => {
                    let fast = sample_or_default(&FAST_WINDOW_SET, idx, variant) as u32;
                    let slow = sample_or_default(&FAST_WINDOW_SET, idx + 3, variant + 2) as u32;
                    let rsi_window = sample_or_default(&RSI_WINDOW_SET, idx, variant);
                    let rsi_oversold = sample_or_default(&RSI_OVERSOLD_SET, idx, variant * 2);
                    let rsi_overbought = sample_or_default(&RSI_OVERBOUGHT_SET, idx, variant + 1);
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        fast.max(2),
                        slow.max(fast + 2),
                        Some(rsi_window),
                        Some(rsi_oversold),
                        Some(rsi_overbought),
                        None,
                        None,
                        variant,
                    ));
                }
                RULE_VOL_REGIME => {
                    let fast = sample_or_default(&FAST_WINDOW_SET, idx, variant);
                    let slow = sample_or_default(&SLOW_WINDOW_SET, idx + 1, variant * 2);
                    let vol_window = sample_or_default(&VOL_WINDOW_SET, idx, variant + 1);
                    let vol_cap = sample_or_default(&VOL_CAP_SET, idx, variant + 2);
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        fast,
                        slow,
                        None,
                        None,
                        None,
                        Some(vol_window),
                        Some(vol_cap),
                        variant,
                    ));
                }
                _ => {}
            }
        }
    }

    out
}

fn build_deterministic_grid_candidates(min_candidates: usize) -> Vec<Candidate> {
    let mut out = Vec::new();
    let assets = ["SPY", "QQQ", "SPXL", "IWM", "TQQQ"];
    let mut id = 0usize;

    for rule in [RULE_TREND_MOMENTUM, RULE_TREND_PULLBACK, RULE_RSI_REVERSION, RULE_VOL_REGIME] {
        for asset_idx in 0..assets.len() {
            for i in 0..FAST_WINDOW_SET.len() {
                for j in 0..SLOW_WINDOW_SET.len() {
                    let fast = FAST_WINDOW_SET[i];
                    let slow = SLOW_WINDOW_SET[j];
                    if (rule == RULE_TREND_MOMENTUM || rule == RULE_TREND_PULLBACK) && slow <= fast {
                        continue;
                    }
                    if out.len() >= min_candidates.saturating_mul(2) {
                        return out;
                    }
                    let candidate = match rule {
                        RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => seed_candidate(
                            0,
                            &ExaSeed {
                                title: "deterministic grid".to_string(),
                                url: String::new(),
                                text: String::new(),
                            },
                            rule,
                            assets[asset_idx],
                            fast,
                            slow,
                            None,
                            None,
                            None,
                            None,
                            None,
                            id,
                        ),
                        RULE_RSI_REVERSION => {
                            let rsi_window = RSI_WINDOW_SET[(i + j) % RSI_WINDOW_SET.len()];
                            let rsi_oversold =
                                RSI_OVERSOLD_SET[(i + asset_idx) % RSI_OVERSOLD_SET.len()];
                            let rsi_overbought =
                                RSI_OVERBOUGHT_SET[(j + asset_idx) % RSI_OVERBOUGHT_SET.len()];
                            let mut args = vec![
                                "--asset".to_string(),
                                assets[asset_idx].to_string(),
                                "--rule".to_string(),
                                rule.to_string(),
                                "--rsi-window".to_string(),
                                rsi_window.to_string(),
                                "--rsi-oversold".to_string(),
                                rsi_oversold.to_string(),
                                "--rsi-overbought".to_string(),
                                rsi_overbought.to_string(),
                                "--fast-window".to_string(),
                                fast.to_string(),
                                "--slow-window".to_string(),
                                slow.to_string(),
                            ];
                            args.extend(vec![
                                "--hypothesis-id".to_string(),
                                format!("grid-{id}"),
                            ]);
                            Candidate {
                                candidate_id: format!("grid-rsi-{id}"),
                                strategy: RESEARCH_STRATEGY.to_string(),
                                rule: rule.to_string(),
                                args,
                                rationale: "Deterministic paper-research RSI reversion variant (grid fallback)".to_string(),
                                source: "deterministic-grid".to_string(),
                                focus_asset: assets[asset_idx].to_string(),
                                is_seeded: false,
                                _min_observations: 20,
                                _min_signals: 10,
                            }
                        }
                        RULE_VOL_REGIME => {
                            let vol_window = VOL_WINDOW_SET[(i + j) % VOL_WINDOW_SET.len()];
                            let vol_cap = VOL_CAP_SET[(i + asset_idx) % VOL_CAP_SET.len()];
                            let mut args = vec![
                                "--asset".to_string(),
                                assets[asset_idx].to_string(),
                                "--rule".to_string(),
                                rule.to_string(),
                                "--vol-window".to_string(),
                                vol_window.to_string(),
                                "--vol-cap".to_string(),
                                format!("{:.2}", vol_cap),
                                "--fast-window".to_string(),
                                fast.to_string(),
                                "--slow-window".to_string(),
                                slow.to_string(),
                            ];
                            args.extend(vec![
                                "--hypothesis-id".to_string(),
                                format!("grid-{id}"),
                            ]);
                            Candidate {
                                candidate_id: format!("grid-vol-{id}"),
                                strategy: RESEARCH_STRATEGY.to_string(),
                                rule: rule.to_string(),
                                args,
                                rationale: "Deterministic paper-research volatility-regime variant (grid fallback)".to_string(),
                                source: "deterministic-grid".to_string(),
                                focus_asset: assets[asset_idx].to_string(),
                                is_seeded: false,
                                _min_observations: 20,
                                _min_signals: 10,
                            }
                        }
                        _ => unreachable!(),
                    };
                    id = id.saturating_add(1);
                    out.push(candidate);
                }
            }
        }
    }

    out
}

fn build_candidate_pool(
    seed_ideas: &[ExaSeed],
    seed_web: bool,
    include_grid: bool,
    min_candidates: usize,
) -> Vec<Candidate> {
    let mut pool = Vec::new();
    let mut seen = HashSet::new();

    let push = |candidate: Candidate, pool: &mut Vec<Candidate>, seen: &mut HashSet<String>| {
        let sig = signature_for_candidate(&candidate);
        if seen.insert(sig) {
            pool.push(candidate);
        }
    };

    if seed_web && !seed_ideas.is_empty() {
        for (idx, seed) in seed_ideas.iter().enumerate() {
            let candidates = build_seed_candidates(seed, idx + 1);
            for candidate in candidates {
                push(candidate, &mut pool, &mut seen);
            }
        }
    }

    if include_grid || pool.len() < min_candidates {
        let target = min_candidates.saturating_sub(pool.len());
        for mut candidate in build_deterministic_grid_candidates(target.max(1)) {
            candidate.is_seeded = false;
            candidate.source = "deterministic-grid".to_string();
            push(candidate, &mut pool, &mut seen);
        }
    }

    if pool.is_empty() {
        return build_deterministic_grid_candidates(min_candidates.max(MIN_CANDIDATES_TARGET_DEFAULT));
    }

    pool
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

fn score_paper_research(payload: &Value) -> Option<ScoredRun> {
    let rows = payload.get("results")?.as_array()?;
    if rows.is_empty() {
        return None;
    }

    let row = rows
        .iter()
        .find(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase()
                != "buy & hold"
        })
        .or_else(|| rows.first())?;

    let cagr = row.get("cagr").and_then(safe_float)?;
    let sharpe = row.get("sharpe").and_then(safe_float)?;
    let dd = row.get("max_drawdown").and_then(safe_float)?;
    let var95 = row.get("var_95").and_then(safe_float).unwrap_or(0.0);
    let final_equity = row.get("final_equity").and_then(safe_float).unwrap_or(0.0);

    let score = 6.0 * cagr + 3.0 * sharpe - 1.4 * dd.abs() - 0.5 * var95.max(0.0);
    Some(ScoredRun {
        score,
        details: json!({
            "name": row.get("name"),
            "final_equity": final_equity,
            "cagr": cagr,
            "sharpe": sharpe,
            "max_drawdown": dd,
            "var_95": var95,
        }),
    })
}

fn score_payload(payload: &Value) -> Option<ScoredRun> {
    score_paper_research(payload)
}

fn strategy_category(strategy: &str) -> &'static str {
    if strategy == RESEARCH_STRATEGY {
        "Research"
    } else {
        "Other"
    }
}

fn run_candidate(
    candidate: &Candidate,
    doob_bin: &Path,
    end_date: &str,
    sessions: i64,
    stage: &str,
    verbose: bool,
) -> Option<ScoredRun> {
    let mut args = vec![
        "--output".to_string(),
        "json".to_string(),
        "run".to_string(),
        candidate.strategy.clone(),
    ];
    args.extend(candidate.args.iter().cloned());
    args.push("--end-date".to_string());
    args.push(end_date.to_string());
    args.push("--sessions".to_string());
    args.push(sessions.to_string());

    if verbose {
        println!("  [{stage}] {}", format_command_line(doob_bin, &args));
    }

    let mut cmd = Command::new(doob_bin);
    cmd.args(&args);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        if verbose {
            eprintln!("  doob failed (code={:?})", output.status.code().unwrap_or(-1));
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

    score_payload(&payload)
}

fn format_detail_summary(details: &Value) -> String {
    let cagr = safe_float(details.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let sharpe = safe_float(details.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let dd = safe_float(details.get("max_drawdown").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let var95 = safe_float(details.get("var_95").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let equity = safe_float(details.get("final_equity").unwrap_or(&Value::Null)).unwrap_or(0.0);
    format!(
        "name={} cagr={:.3} sharpe={:.3} dd={:.2}% var95={:.3} final_equity={:.2}",
        details.get("name").and_then(Value::as_str).unwrap_or("?"),
        cagr,
        sharpe,
        dd,
        var95,
        equity
    )
}

fn profitability_blurb(train: &Value, test: &Value, strategy: &str, rule: &str, args: &[String], asset: &str) -> String {
    let train_cagr = safe_float(train.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let test_cagr = safe_float(test.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let train_sharpe = safe_float(train.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let test_sharpe = safe_float(test.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let train_dd = safe_float(train.get("max_drawdown").unwrap_or(&Value::Null)).unwrap_or(0.0).abs();
    let test_dd = safe_float(test.get("max_drawdown").unwrap_or(&Value::Null)).unwrap_or(0.0).abs();
    let signal = rule_description(rule, args, asset);
    let mechanics = format!(
        "Why it works: {} This strategy is run through `{}` in `doob`.",
        signal,
        strategy
    );

    if train_cagr > 0.05 && test_cagr > 0.02 && train_sharpe > 0.2 && test_sharpe > 0.1 {
        format!(
            "{} Train: CAGR {}, Sharpe {}, max drawdown {}. Test: CAGR {}, Sharpe {}, max drawdown {}. Strongly consistent equity growth across both windows.",
            mechanics,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd),
        )
    } else if train_sharpe > 0.0 && test_sharpe > 0.0 {
        format!(
            "{} Train: CAGR {}, Sharpe {} (drawdown {}). Test: CAGR {}, Sharpe {} (drawdown {}). Risk-adjusted edge is present in both windows but less stable than ideal.",
            mechanics,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd),
        )
    } else if test_cagr > 0.0 && test_sharpe > 0.0 {
        format!(
            "{} Train: CAGR {}, Sharpe {} with {} max drawdown. Test: CAGR {}, Sharpe {} with {} max drawdown. The signal appears to adapt better in forward periods; monitor regime dependence.",
            mechanics,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd),
        )
    } else {
        format!(
            "{} Train: CAGR {}, Sharpe {} ({} max drawdown). Test: CAGR {}, Sharpe {} ({} max drawdown). This candidate is not yet clearly robust and should be treated as exploratory.",
            strategy,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd)
        )
    }
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
    if verbose {
        println!(
            "\n{} candidate {}/{}: {}",
            strategy_category(&candidate.strategy),
            idx,
            total,
            candidate.candidate_id
        );
        println!("  strategy: {} | rule: {}", candidate.strategy, candidate.rule);
        println!("  asset: {} | source: {}", candidate.focus_asset, candidate.source);
        println!("  rationale: {}", candidate.rationale);
        println!("  args: {}", candidate.args.join(" "));
        println!("  train window: {} -> {} (sessions={})", train_start, train_end, train_sessions);
    }
    let train_window_sessions = sessions_for_window(train_start, train_end, train_sessions);
    let test_window_sessions = sessions_for_window(test_start, test_end, test_sessions);

    let train_run = run_candidate(
        candidate,
        doob_bin,
        train_end,
        train_window_sessions,
        "train",
        verbose,
    )?;
    if verbose {
        println!("  train score: {:.4}", train_run.score);
        println!("  train summary: {}", format_detail_summary(&train_run.details));
        println!(
            "  test window: {} -> {} (sessions={})",
            test_start, test_end, test_window_sessions
        );
    }

    let test_run = run_candidate(
        candidate,
        doob_bin,
        test_end,
        test_window_sessions,
        "test",
        verbose,
    )?;
    if verbose {
        println!("  test score: {:.4}", test_run.score);
        println!("  test summary: {}", format_detail_summary(&test_run.details));
    }

    let c_train = train_run
        .details
        .get("cagr")
        .and_then(safe_float)
        .unwrap_or(0.0);
    let s_train = train_run
        .details
        .get("sharpe")
        .and_then(safe_float)
        .unwrap_or(0.0);
    let s_test = test_run
        .details
        .get("sharpe")
        .and_then(safe_float)
        .unwrap_or(0.0);
    if c_train <= 0.0 {
        return None;
    }
    if s_train < -3.0 || s_test < -3.0 {
        return None;
    }
    if train_run
        .details
        .get("final_equity")
        .and_then(safe_float)
        .unwrap_or(0.0)
        < 100_000.0
    {
        return None;
    }

    let combined_score = 0.65 * train_run.score + 0.35 * test_run.score;
    Some(CandidateReport {
        candidate_id: candidate.candidate_id.clone(),
        strategy: candidate.strategy.clone(),
        category: strategy_category(&candidate.strategy).to_string(),
        args: candidate.args.clone(),
        rule: candidate.rule.clone(),
        rationale: candidate.rationale.clone(),
        source: candidate.source.clone(),
        focus_asset: candidate.focus_asset.clone(),
        train_score: train_run.score,
        test_score: test_run.score,
        combined_score,
        train_details: train_run.details,
        test_details: test_run.details,
        is_seeded: candidate.is_seeded,
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

fn rank_rows(rows: &[CandidateReport]) -> Vec<CandidateReport> {
    let mut ranked = rows.to_vec();
    ranked.sort_by(|a, b| {
        let seeded = b.is_seeded.cmp(&a.is_seeded);
        if seeded != std::cmp::Ordering::Equal {
            return seeded;
        }
        let score_cmp = b
            .combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal);
        if score_cmp != std::cmp::Ordering::Equal {
            return score_cmp;
        }
        b.train_score
            .partial_cmp(&a.train_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked
}

#[derive(Serialize)]
struct InteractiveRow {
    rank: usize,
    candidate_id: String,
    strategy: String,
    category: String,
    source: String,
    rule: String,
    focus_asset: String,
    args: Vec<String>,
    combined_score: f64,
    train_score: f64,
    test_score: f64,
    train_details: Value,
    test_details: Value,
    why: String,
    is_seeded: bool,
}

fn save_interactive_report(path: &Path, rows: &[CandidateReport]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let top_rows: Vec<InteractiveRow> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| InteractiveRow {
            rank: idx + 1,
            candidate_id: row.candidate_id.clone(),
            strategy: row.strategy.clone(),
            category: row.category.clone(),
            source: row.source.clone(),
            rule: row.rule.clone(),
            focus_asset: row.focus_asset.clone(),
            args: row.args.clone(),
            combined_score: row.combined_score,
            train_score: row.train_score,
            test_score: row.test_score,
            train_details: row.train_details.clone(),
            test_details: row.test_details.clone(),
            why: profitability_blurb(
                &row.train_details,
                &row.test_details,
                &row.strategy,
                &row.rule,
                &row.args,
                &row.focus_asset,
            ),
            is_seeded: row.is_seeded,
        })
        .collect();

    let rows_json = serde_json::to_string_pretty(&top_rows).unwrap_or_else(|_| "[]".to_string());
    let html = format!(
r#"
<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Doob Autoresearch Loop - Top 10 Research Strategies</title>
  <style>
    :root {{
      --bg: #071023;
      --panel: #0f1f3a;
      --muted: #9db2cf;
      --text: #ecf4ff;
      --accent: #60a5fa;
      --ok: #4ade80;
      --warn: #f59e0b;
      --bad: #f87171;
    }}
    body {{ margin: 0; font-family: Inter, system-ui, sans-serif; background: radial-gradient(circle at 30% 0%, #182f54 0%, var(--bg) 45%, #020915 100%); color: var(--text); }}
    .container {{ width: min(1420px, 96vw); margin: 14px auto; }}
    .panel {{ background: linear-gradient(180deg, #102548, #0f1c39); border: 1px solid #21406e; border-radius: 10px; padding: 12px; }}
    h1 {{ margin: 0 0 10px 0; letter-spacing: 0.4px; }}
    .summary {{ display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }}
    .card {{ background: #0f2a51; border: 1px solid #28578f; border-radius: 10px; padding: 10px; }}
    .kpi {{ font-size: 1.8rem; font-weight: 700; }}
    .muted {{ color: var(--muted); }}
    .controls {{ margin: 12px 0; display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 8px; }}
    .controls input, .controls select {{
      padding: 8px;
      border: 1px solid #2f5482;
      border-radius: 8px;
      background: #0e2346;
      color: var(--text);
    }}
    table {{ width: 100%; border-collapse: collapse; }}
    th, td {{ padding: 9px 10px; border-bottom: 1px solid #233c63; text-align: left; vertical-align: top; font-size: 13px; }}
    th {{ background: #17325a; color: #d6e5ff; position: sticky; top: 0; }}
    tr:hover td {{ background: #18345d; }}
    .pill {{ display:inline-block; border:1px solid #365b8e; border-radius:999px; padding:2px 8px; margin-right: 6px; font-size: 11px; }}
    .seed {{ color: var(--ok); }}
    .det {{ color: var(--warn); }}
    .row-details {{ border: 1px solid #27508a; border-radius: 8px; background: #0f2450; padding: 8px; margin-top: 6px; }}
    .row-details p {{ margin: 4px 0; font-size: 12px; }}
  </style>
</head>
<body>
<div class="container">
  <h1>Doob Autoresearch Loop — Paper-Research Top 10</h1>
  <div class="muted">Net-new strategy discovery from Exa/arXiv paper hypotheses, executed as doob-native `paper-research` candidates.</div>
  <div class="summary">
    <div class="card"><div class="muted">Rows generated</div><div class="kpi">{total}</div></div>
    <div class="card"><div class="muted">Top rows shown</div><div class="kpi">{top_count}</div></div>
    <div class="card"><div class="muted">Seeded in top10</div><div class="kpi">{seeded_top}</div></div>
  </div>
  <div class="panel">
    <div class="controls">
      <input id="search" placeholder="Search strategy, rule, source, asset, rationale">
      <select id="cat"><option value="All">All Categories</option><option value="Research">Research</option></select>
      <select id="src"><option value="All">All Sources</option></select>
      <select id="seeded">
        <option value="All">All</option>
        <option value="Seeded">Seeded</option>
        <option value="Fallback">Fallback</option>
      </select>
    </div>
    <div style="overflow:auto; max-height: 68vh;">
      <table>
        <thead>
          <tr>
            <th>Rank</th>
            <th>Candidate</th>
            <th>Rule</th>
            <th>Category</th>
            <th>Score</th>
            <th>Train</th>
            <th>Test</th>
            <th>Source</th>
            <th>Why profitable?</th>
          </tr>
        </thead>
        <tbody id="body"></tbody>
      </table>
    </div>
    <div id="details"></div>
  </div>
</div>
<script>
const rows = {rows_json};
const body = document.getElementById('body');
const details = document.getElementById('details');
const search = document.getElementById('search');
const cat = document.getElementById('cat');
const src = document.getElementById('src');
const seeded = document.getElementById('seeded');

function fmt(v) {{
  return (typeof v === 'number' && Number.isFinite(v))
    ? v.toLocaleString(undefined, {{ maximumFractionDigits: 3, minimumFractionDigits: 3 }})
    : 'N/A';
}}
function fmtPct(v) {{
  return (typeof v === 'number' && Number.isFinite(v))
    ? (v * 100).toLocaleString(undefined, {{ maximumFractionDigits: 2, minimumFractionDigits: 2 }}) + '%'
    : 'N/A';
}}

function pickMetric(row, key) {{
  const t = row.train_details || {{}};
  const s = row.test_details || {{}};
  const values = [t[key], s[key]];
  return values;
}}

const allSources = ['All', ...new Set(rows.map(r => r.source))];
src.innerHTML = allSources.map(v => `<option value="${{v}}">${{v}}</option>`).join('');

function render() {{
  const query = search.value.trim().toLowerCase();
  const catV = cat.value;
  const srcV = src.value;
  const seededV = seeded.value;

  let filtered = rows.filter(r => {{
    if (catV !== 'All' && r.category !== catV) return false;
    if (srcV !== 'All' && r.source !== srcV) return false;
    if (seededV === 'Seeded' && !r.is_seeded) return false;
    if (seededV === 'Fallback' && r.is_seeded) return false;
    if (!query) return true;
    const hay = `${{r.candidate_id}} ${{r.strategy}} ${{r.rule}} ${{r.source}} ${{r.focus_asset}}`.toLowerCase();
    return hay.includes(query);
  }});

  filtered.sort((a, b) => (b.combined_score - a.combined_score));
  const top10 = filtered.slice(0, 10);
  body.innerHTML = top10.map((r, idx) => {{
    const type = r.is_seeded ? '<span class=\"pill seed\">seeded</span>' : '<span class=\"pill det\">fallback</span>';
    const train = fmt(r.train_score);
    const ttest = fmt(r.test_score);
    const [trCagr, teCagr] = [pickMetric(r, 'cagr')[0], pickMetric(r, 'cagr')[1]];
    const [trSharpe, teSharpe] = [pickMetric(r, 'sharpe')[0], pickMetric(r, 'sharpe')[1]];
    const [trDD, teDD] = [pickMetric(r, 'max_drawdown')[0], pickMetric(r, 'max_drawdown')[1]];
    return `
      <tr>
        <td>${{idx + 1}}</td>
        <td><b>${{r.candidate_id}}</b> ${{type}}<div class="muted">${{r.focus_asset}}</div><code>${{r.args.join(' ')}}</code></td>
        <td>${{r.rule}}</td>
        <td><span class="pill">${{r.category}}</span></td>
        <td>${{fmt(r.combined_score)}}</td>
        <td>${{trCagr !== 'N/A' ? fmtPct(trCagr) : 'N/A'}} / S:${{fmt(trSharpe)}}</td>
        <td>${{teCagr !== 'N/A' ? fmtPct(teCagr) : 'N/A'}} / S:${{fmt(teSharpe)}}</td>
        <td>${{r.source}}</td>
        <td>${{r.why}}</td>
      </tr>
      <tr><td colspan="9">
        <div class="row-details">
          <p><span class="muted">Drawdowns:</span> train ${{fmtPct(trDD || 0)}} / test ${{fmtPct(teDD || 0)}}</p>
          <p><strong>Why:</strong> ${{r.why}}</p>
        </div>
      </td></tr>
    `;
  }}).join('');

  details.innerHTML = top10.map((r, idx) => `
    <details>
      <summary><b>${{idx + 1}}. ${{r.candidate_id}}</b> — ${{r.strategy}} (${{r.rule}})</summary>
      <p><span class="muted">Source:</span> ${{r.source}} | Asset: ${{r.focus_asset}}</p>
      <p><span class="muted">Args:</span> <code>${{r.args.join(' ')}}</code></p>
      <p><strong>Why profitable:</strong> ${{r.why}}</p>
    </details>
  `).join('');
}}

search.addEventListener('input', render);
cat.addEventListener('change', render);
src.addEventListener('change', render);
seeded.addEventListener('change', render);

render();
</script>
</body>
</html>
"#,
        rows_json = rows_json,
        total = rows.len(),
        top_count = rows.len().min(10),
        seeded_top = rows.iter().take(10).filter(|r| r.is_seeded).count()
    );

    std::fs::write(path, html)
}

fn save_ledger(path: &Path, rows: &[CandidateReport]) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::options().append(true).create(true).open(path)?;
    let ts = Utc::now().to_rfc3339();
    for row in rows {
        let record = json!({
            "timestamp": ts,
            "candidate_id": row.candidate_id,
            "strategy": row.strategy,
            "category": row.category,
            "rule": row.rule,
            "args": row.args,
            "source": row.source,
            "focus_asset": row.focus_asset,
            "train_score": row.train_score,
            "test_score": row.test_score,
            "combined_score": row.combined_score,
            "is_seeded": row.is_seeded,
            "train_details": row.train_details,
            "test_details": row.test_details,
        });
        writeln!(file, "{}", serde_json::to_string(&record).unwrap_or_else(|_| "{}".to_string()))?;
    }
    Ok(())
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

fn print_top(rows: &[CandidateReport], k: usize) {
    let rows = rank_rows(rows);
    println!("\nTop candidates:");
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "rank",
        "score",
        "seeded",
        "category",
        "strategy",
        "rule",
        "source",
        "asset",
        "train",
        "test",
    ]);
    for (i, row) in rows.iter().take(k).enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            format!("{:.3}", row.combined_score),
            if row.is_seeded { "seed".to_string() } else { "fallback".to_string() },
            row.category.clone(),
            row.strategy.clone(),
            row.rule.clone(),
            row.source.clone(),
            row.focus_asset.clone(),
            format!("{:.4}", row.train_score),
            format!("{:.4}", row.test_score),
        ]);
    }
    println!("{}", table);
}

fn print_best(best: &CandidateReport) {
    println!("\nBest candidate:");
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["field", "value"]);
    table.add_row(vec!["strategy".to_string(), best.strategy.clone()]);
    table.add_row(vec!["rule".to_string(), best.rule.clone()]);
    table.add_row(vec!["candidate_id".to_string(), best.candidate_id.clone()]);
    table.add_row(vec!["category".to_string(), best.category.clone()]);
    table.add_row(vec!["asset".to_string(), best.focus_asset.clone()]);
    table.add_row(vec![
        "score".to_string(),
        format!("{:.4}", best.combined_score),
    ]);
    table.add_row(vec![
        "train/test".to_string(),
        format!("{:.4} / {:.4}", best.train_score, best.test_score),
    ]);
    table.add_row(vec!["source".to_string(), best.source.clone()]);
    table.add_row(vec!["command".to_string(), format!("doob --output json run {} {}", best.strategy, best.args.join(" "))]);
    table.add_row(vec!["rationale".to_string(), best.rationale.clone()]);
    table.add_row(vec!["train details".to_string(), format_detail_summary(&best.train_details)]);
    table.add_row(vec!["test details".to_string(), format_detail_summary(&best.test_details)]);
    println!("{}", table);
}

fn main() {
    let _ = dotenv();
    let args = Args::parse();
    let doob_bin = PathBuf::from(&args.doob_bin);
    if !doob_bin.exists() {
        eprintln!("doob binary not found: {}", doob_bin.display());
        return;
    }

    println!(
        "Autoresearch run: paper-research only | strategy seed web: {} | candidates up to {}",
        args.seed_web,
        args.candidates
    );
    println!(
        "Train window: {} -> {} (fallback sessions {}), Test window: {} -> {} (fallback sessions {})",
        args.train_start,
        args.train_end,
        args.train_sessions,
        args.test_start,
        args.test_end,
        args.test_sessions
    );

    let seed_ideas = if args.seed_web {
        let ideas = fetch_exa_ideas(SEED_QUERIES, 25);
        if args.verbose {
            println!("fetched {} arXiv seeds from Exa", ideas.len());
        }
        if let Err(err) = save_seed_ideas(Path::new("reports/autoresearch-exa-ideas.json"), &ideas) {
            eprintln!("Failed to write exa seed file: {err}");
        }
        ideas
    } else {
        Vec::new()
    };

    let mut candidates = build_candidate_pool(
        &seed_ideas,
        args.seed_web,
        args.include_grid || !args.seed_web,
        args.candidates,
    );

    if candidates.is_empty() {
        eprintln!("No candidates were generated; check data source and network key");
        return;
    }
    if candidates.len() < args.candidates {
        if args.verbose {
            println!(
                "expanded deterministic pool from {} to {} candidates to satisfy requested count",
                candidates.len(),
                args.candidates
            );
        }
    }

    shuffle_candidates(&mut candidates, args.random_seed);
    if candidates.len() > args.candidates {
        candidates.truncate(args.candidates);
    }

    if args.verbose {
        println!("candidate pool: {} total", candidates.len());
        let seeded_count = candidates.iter().filter(|r| r.is_seeded).count();
        println!("seeded: {} / fallback: {}", seeded_count, candidates.len() - seeded_count);
    }

    let train_window_sessions = sessions_for_window(&args.train_start, &args.train_end, args.train_sessions);
    let test_window_sessions = sessions_for_window(&args.test_start, &args.test_end, args.test_sessions);
    let mut results = Vec::new();

    for (idx, candidate) in candidates.iter().enumerate() {
        if let Some(report) = evaluate_candidate(
            candidate,
            idx + 1,
            candidates.len(),
            &doob_bin,
            &args.train_start,
            &args.train_end,
            &args.test_start,
            &args.test_end,
            train_window_sessions,
            test_window_sessions,
            args.verbose,
        ) {
            results.push(report);
        } else if args.verbose {
            println!("  rejected: scoring gate failed or doob execution error");
        }
    }

    if args.verbose {
        println!("completed loop: {} candidates passed", results.len());
    }
    if results.is_empty() {
        println!("No candidate passed scoring gates.");
        return;
    }

    let ranked = rank_rows(&results);
    let top_k = args.top.min(ranked.len());
    print_top(&ranked, top_k);
    if let Some(best) = ranked.first() {
        print_best(best);
    }
    if let Err(err) = save_interactive_report(
        Path::new("reports/autoresearch-top10-interactive-report.html"),
        &ranked.iter().take(10).cloned().collect::<Vec<_>>(),
    ) {
        eprintln!("Failed to write interactive report: {err}");
    }
    if let Err(err) = save_ledger(
        Path::new("reports/autoresearch-ledger.jsonl"),
        &ranked.iter().take(10).cloned().collect::<Vec<_>>(),
    ) {
        eprintln!("Failed to save ledger: {err}");
    }

    let _ = Command::new("open")
        .arg("reports/autoresearch-top10-interactive-report.html")
        .spawn();
}
