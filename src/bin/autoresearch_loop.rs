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
use dotenvy::dotenv;

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

const SEED_RESULTS_PER_QUERY: usize = 20;
const HORIZON_1W: &str = "1w=5";
const HORIZON_1M: &str = "1m=21";
const HORIZON_2W: &str = "2w=10";
const HORIZON_3M: &str = "3m=63";

const NOVEL_THRESHOLD_STEP: i32 = 5;
const NOVEL_LOOKBACK_STEP: i32 = 1;

const NOVEL_MIN_THRESHOLD: f64 = 35.0;
const NOVEL_MAX_THRESHOLD: f64 = 90.0;
const NOVEL_MIN_LOOKBACK: u32 = 3;
const NOVEL_MAX_LOOKBACK: u32 = 60;
const NOVEL_MAX_LONG: u32 = 260;

fn select_str<'a>(items: &'a [&'a str], idx: usize, salt: usize) -> &'a str {
    items[(idx + salt) % items.len()]
}

fn select_u32(items: &[u32], idx: usize, salt: usize) -> u32 {
    items[(idx + salt) % items.len()]
}

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
    is_seeded: bool,
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
    is_seeded: bool,
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

    #[arg(long, default_value_t = 100)]
    candidates: usize,

    #[arg(long, default_value_t = 20)]
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

fn has_arg(args: &[String], flag: &str) -> bool {
    args.iter().any(|v| v == flag)
}

fn set_arg_value(args: &mut [String], flag: &str, value: &str) -> bool {
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            args[i + 1] = value.to_string();
            return true;
        }
    }
    false
}

fn mutate_u32_arg(args: &mut [String], flag: &str, delta: i32, min: u32, max: u32) -> bool {
    for i in 0..args.len().saturating_sub(1) {
        if args[i] != flag {
            continue;
        }
        let Ok(value) = args[i + 1].parse::<i32>() else {
            continue;
        };
        let min = min as i32;
        let max = max as i32;
        let new_value = (value + delta).clamp(min, max).to_string();
        if args[i + 1] != new_value {
            args[i + 1] = new_value;
            return true;
        }
        return false;
    }
    false
}

fn mutate_f64_arg(args: &mut [String], flag: &str, delta: f64, min: f64, max: f64) -> bool {
    for i in 0..args.len().saturating_sub(1) {
        if args[i] != flag {
            continue;
        }
        let Ok(value) = args[i + 1].parse::<f64>() else {
            continue;
        };
        let new_value = (value + delta).clamp(min, max);
        let new_value = if new_value.fract() == 0.0 {
            format!("{}", new_value as i32)
        } else {
            format!("{:.2}", new_value)
        };
        if args[i + 1] != new_value {
            args[i + 1] = new_value;
            return true;
        }
        return false;
    }
    false
}

fn is_breadth_strategy(strategy: &str) -> bool {
    BREADTH_STRATEGIES.contains(&strategy)
}

fn signature_for_candidate(candidate: &Candidate) -> String {
    format!("{}|{}", candidate.strategy, candidate.args.join("|"))
}

fn insert_if_novel(
    pool: &mut Vec<Candidate>,
    signature_cache: &mut HashSet<String>,
    known_legacy: &HashSet<String>,
    candidate: Candidate,
) -> bool {
    let sig = signature_for_candidate(&candidate);
    if known_legacy.contains(&sig) {
        return false;
    }
    if signature_cache.insert(sig) {
        pool.push(candidate);
        return true;
    }
    false
}

fn build_known_signature_set() -> HashSet<String> {
    let mut signatures = HashSet::new();
    for candidate in build_grid_candidate_pool() {
        signatures.insert(signature_for_candidate(&candidate));
    }
    signatures
}

fn emit_seed_mutation(mutant_base: &Candidate, seed_idx: usize, variant_offset: usize) -> Option<Candidate> {
    if variant_offset == 0 {
        return Some(mutant_base.clone());
    }
    mutate_seed_candidate(mutant_base, seed_idx, variant_offset)
}

fn mutate_seed_candidate(base: &Candidate, seed_idx: usize, variant: usize) -> Option<Candidate> {
    let mut args = base.args.clone();
    let mut changed = false;

    if is_breadth_strategy(base.strategy.as_str()) {
        if !has_arg(&args, "--price-returns") {
            args.push("--price-returns".to_string());
            changed = true;
        }
        if !has_arg(&args, "--max-workers") {
            args.push("--max-workers".to_string());
            args.push("8".to_string());
            changed = true;
        }

        let horizon_cycle = [HORIZON_1W, HORIZON_1M, HORIZON_2W, HORIZON_3M];
        let alt_horizon = horizon_cycle[(seed_idx + variant) % horizon_cycle.len()];
        if set_arg_value(&mut args, "--horizon", alt_horizon) {
            changed = true;
        }

        if mutate_u32_arg(&mut args, "--lookback", NOVEL_LOOKBACK_STEP, NOVEL_MIN_LOOKBACK, NOVEL_MAX_LOOKBACK) {
            changed = true;
        }
        if mutate_u32_arg(&mut args, "--short-period", NOVEL_LOOKBACK_STEP, NOVEL_MIN_LOOKBACK, NOVEL_MAX_LOOKBACK) {
            changed = true;
        }
        if mutate_u32_arg(&mut args, "--long-period", -(NOVEL_LOOKBACK_STEP * 2), NOVEL_MAX_LONG / 2, NOVEL_MAX_LONG) {
            changed = true;
        }
        if mutate_f64_arg(
            &mut args,
            "--threshold",
            if variant % 2 == 0 { NOVEL_THRESHOLD_STEP as f64 } else { -(NOVEL_THRESHOLD_STEP as f64) },
            NOVEL_MIN_THRESHOLD,
            NOVEL_MAX_THRESHOLD,
        ) {
            changed = true;
        }
    } else if base.strategy == "intraday-drift" {
        if variant % 2 == 0 && !has_arg(&args, "--short") {
            args.push("--short".to_string());
            changed = true;
        }
        if !has_arg(&args, "--start-year-table") {
            args.push("--start-year-table".to_string());
            args.push(if variant % 2 == 0 { "2017".to_string() } else { "2016".to_string() });
            changed = true;
        }
        if !has_arg(&args, "--capital") {
            args.push("--capital".to_string());
            args.push("750000".to_string());
            changed = true;
        }
    } else if base.strategy == "overnight-drift" {
        if !has_arg(&args, "--no-vix-filter") && variant % 3 == 0 {
            args.push("--no-vix-filter".to_string());
            changed = true;
        }
        if !has_arg(&args, "--capital") {
            args.push("--capital".to_string());
            args.push("1100000".to_string());
            changed = true;
        }
        if !has_arg(&args, "--start-year-table") {
            args.push("--start-year-table".to_string());
            args.push("2017".to_string());
            changed = true;
        }
    }

    if !changed {
        return None;
    }

    Some(Candidate {
        candidate_id: format!("{}-novel-{}", base.candidate_id, variant),
        strategy: base.strategy.clone(),
        args,
        rationale: format!("Novel mutant from web-seed idea: {}", base.rationale),
        source: format!("{}#novel", base.source),
        focus_assets: base.focus_assets.clone(),
        target_horizon: if is_breadth_strategy(base.strategy.as_str()) {
            let alt_horizon = [HORIZON_1W, HORIZON_1M, HORIZON_2W, HORIZON_3M][(seed_idx + variant) % 4];
            alt_horizon.to_string()
        } else {
            base.target_horizon.clone()
        },
        min_observations: base.min_observations,
        min_signals: base.min_signals,
        is_diagnostic: false,
        is_seeded: true,
    })
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
    if contains_any(&blob, &["breakout", "swing", "momentum breakout"]) {
        tags.insert("breakout");
    }
    if contains_any(&blob, &["liquidity", "volume", "volume profile", "market regime", "risk parity", "allocation"]) {
        tags.insert("regime");
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

    let mut out = Vec::new();
    let mut asset_buckets = vec![vec!["SPY", "QQQ"], vec!["SPY", "SPXL"], vec!["SPY", "QQQ", "SPXL"]];
    if title.contains("breakout") || title.contains("S&P") {
        asset_buckets.push(vec!["QQQ", "SPXL", "IWM"]);
    }
    if tags.contains("regime") {
        asset_buckets.push(vec!["SPY", "QQQ", "IWM"]);
        asset_buckets.push(vec!["QQQ", "SPXL", "IWM"]);
    }
    let universe_pool = ["ndx100", "sp500", "r2k", "all-stocks"];
    let washout_lookbacks = [3, 5, 8, 10, 12, 20, 25];
    let dual_short = [2, 5, 8, 10, 15, 20];
    let dual_long = [20, 50, 100, 150, 200];
    let dual_thresholds = [8, 10, 12, 15, 18, 20, 25, 30];
    let ma_lookbacks = [8, 20, 35, 50];
    let ma_thresholds = [45, 50, 55, 60, 65, 70, 75, 80];
    let modes = ["oversold", "overbought"];
    let horizons = [HORIZON_1M, HORIZON_1W];

    let asset_set_for = |slot: usize| -> Vec<String> {
        asset_buckets[slot % asset_buckets.len()]
            .iter()
            .map(|s| s.to_string())
            .collect()
    };

    if tags.contains("momentum") || tags.contains("breadth") {
        for variant in 0..2 {
            let lookback = select_u32(&washout_lookbacks, idx, variant).to_string();
            let threshold = select_u32(&ma_thresholds, idx, variant * 2).to_string();
            let mode = select_str(&modes, idx, variant);
            let universe = select_str(&universe_pool, idx, variant + 1);
            let assets = asset_set_for(variant);
            let mut args = vec![
                "--universe".to_string(),
                universe.to_string(),
                "--lookback".to_string(),
                lookback,
                "--signal-mode".to_string(),
                mode.to_string(),
                "--threshold".to_string(),
                threshold,
                "--assets".to_string(),
            ];
            args.extend(assets.iter().map(|s| s.to_string()));
            push_horizon(&mut args, select_str(&horizons, idx, variant));

            out.push(Candidate {
                candidate_id: format!("seed-mom-{idx:02}a{variant}"),
                strategy: "breadth-washout".to_string(),
                args,
                rationale: format!("Web-evolved momentum mutant: {title}"),
                source: source.clone(),
                focus_assets: assets.clone(),
                target_horizon: select_str(&horizons, idx, variant).to_string(),
                min_observations: 18,
                min_signals: 8,
                is_diagnostic: false,
                is_seeded: true,
            });

            let short = select_u32(&dual_short, idx, variant + 2).to_string();
            let long = select_u32(&dual_long, idx, variant + 4).to_string();
            let dual_threshold = select_u32(&dual_thresholds, idx, variant + 1).to_string();
            let dual_assets = if assets.len() >= 2 { assets.clone() } else { vec!["SPY".to_string(), "QQQ".to_string()] };
            let mut dual_args = vec![
                "--short-period".to_string(),
                short,
                "--long-period".to_string(),
                long,
                "--threshold".to_string(),
                dual_threshold,
                "--universe".to_string(),
                select_str(&universe_pool, idx, variant + 4).to_string(),
                "--assets".to_string(),
            ];
            dual_args.extend(dual_assets.iter().map(|s| s.to_string()));
            push_horizon(&mut dual_args, select_str(&horizons, idx, variant + 1));

            out.push(Candidate {
                candidate_id: format!("seed-mom-alt-{idx:02}b{variant}"),
                strategy: "breadth-dual-ma".to_string(),
                args: dual_args,
                rationale: format!("Web-evolved momentum/mean-reversion mutant: {title}"),
                source: source.clone(),
                focus_assets: dual_assets,
                target_horizon: select_str(&horizons, idx, variant + 1).to_string(),
                min_observations: 20,
                min_signals: 10,
                is_diagnostic: false,
                is_seeded: true,
            });
        }
    }

    if tags.contains("regime") {
        for variant in 0..2 {
            let short = select_u32(&dual_short, idx, variant + 3);
            let long = select_u32(&dual_long, idx, variant + 1);
            if short >= long {
                continue;
            }
            let threshold = select_u32(&dual_thresholds, idx, variant + 2);
            let regime_assets = asset_set_for(variant + 1);
            let mut args = vec![
                "--short-period".to_string(),
                short.to_string(),
                "--long-period".to_string(),
                long.to_string(),
                "--threshold".to_string(),
                threshold.to_string(),
                "--universe".to_string(),
                select_str(&universe_pool, idx, variant + 2).to_string(),
                "--assets".to_string(),
            ];
            args.extend(regime_assets.iter().map(|s| s.to_string()));
            push_horizon(&mut args, select_str(&horizons, idx, variant + 3));

                out.push(Candidate {
                    candidate_id: format!("seed-reg-{idx:02}a{variant}"),
                    strategy: "breadth-dual-ma".to_string(),
                    args,
                    rationale: format!("Web-evolved regime mutant: {title}"),
                    source: source.clone(),
                    focus_assets: regime_assets,
                    target_horizon: HORIZON_1M.to_string(),
                    min_observations: 20,
                    min_signals: 8,
                    is_diagnostic: false,
                    is_seeded: true,
                });

            let washout_assets = asset_set_for(variant + 2);
            let lookback = select_u32(&washout_lookbacks, idx, variant + 1).to_string();
            let threshold = select_u32(&ma_thresholds, idx, variant + 3).to_string();
            let mode = select_str(&modes, idx, variant + 1);
            let mut washout_args = vec![
                "--universe".to_string(),
                select_str(&universe_pool, idx, variant + 3).to_string(),
                "--lookback".to_string(),
                lookback,
                "--signal-mode".to_string(),
                mode.to_string(),
                "--threshold".to_string(),
                threshold,
                "--assets".to_string(),
            ];
            washout_args.extend(washout_assets.iter().map(|s| s.to_string()));
            push_horizon(&mut washout_args, HORIZON_1M);

                out.push(Candidate {
                    candidate_id: format!("seed-reg-{idx:02}b{variant}"),
                    strategy: "breadth-washout".to_string(),
                    args: washout_args,
                    rationale: format!("Web-evolved regime/transition mutant: {title}"),
                    source: source.clone(),
                    focus_assets: washout_assets,
                    target_horizon: HORIZON_1M.to_string(),
                    min_observations: 18,
                    min_signals: 8,
                    is_diagnostic: false,
                    is_seeded: true,
                });
        }
    }

    if tags.contains("reversion") {
        for variant in 0..3 {
            let lookback = select_u32(&ma_lookbacks, idx, variant + 2).to_string();
            let mode = select_str(&modes, idx, variant + 1);
            let threshold = select_u32(&ma_thresholds, idx, variant).to_string();
            let rev_assets = asset_set_for(variant + 2);
            let mut args = vec![
                "--short-period".to_string(),
                lookback,
                "--signal-mode".to_string(),
                mode.to_string(),
                "--threshold".to_string(),
                threshold,
                "--universe".to_string(),
                select_str(&universe_pool, idx, variant + 1).to_string(),
                "--assets".to_string(),
            ];
            args.extend(rev_assets.iter().map(|s| s.to_string()));
            push_horizon(&mut args, if variant % 2 == 0 { HORIZON_1W } else { HORIZON_1M });

            out.push(Candidate {
                candidate_id: format!("seed-rev-{idx:02}a{variant}"),
                strategy: "breadth-ma".to_string(),
                args,
                rationale: format!("Web-evolved reversion mutant: {title}"),
                source: source.clone(),
                focus_assets: rev_assets.clone(),
                target_horizon: if variant % 2 == 0 {
                    HORIZON_1W.to_string()
                } else {
                    HORIZON_1M.to_string()
                },
                min_observations: 20,
                min_signals: 8,
                is_diagnostic: false,
                is_seeded: true,
            });

            let dual_assets = if rev_assets.len() >= 2 { rev_assets } else { vec!["SPY".to_string(), "QQQ".to_string()] };
            let short = select_u32(&dual_short, idx, variant + 4).to_string();
            let long = select_u32(&dual_long, idx, variant + 5).to_string();
            if short < long {
                let mut args = vec![
                    "--short-period".to_string(),
                    short,
                    "--long-period".to_string(),
                    long,
                    "--threshold".to_string(),
                    select_u32(&dual_thresholds, idx, variant + 6).to_string(),
                    "--universe".to_string(),
                    select_str(&universe_pool, idx, variant + 3).to_string(),
                    "--assets".to_string(),
                ];
                args.extend(dual_assets.iter().map(|s| s.to_string()));
                push_horizon(&mut args, HORIZON_1W);

                out.push(Candidate {
                    candidate_id: format!("seed-rev-{idx:02}b{variant}"),
                    strategy: "breadth-dual-ma".to_string(),
                    args,
                    rationale: format!("Web-evolved reversion-to-trend mutant: {title}"),
                    source: source.clone(),
                    focus_assets: dual_assets,
                    target_horizon: HORIZON_1W.to_string(),
                    min_observations: 20,
                    min_signals: 8,
                    is_diagnostic: false,
                    is_seeded: true,
                });
            }
        }
    }

    if tags.contains("intraday") {
        let intraday_assets = ["QQQ", "SPY", "SPXL"];
        for variant in 0..2 {
            let ticker = select_str(&intraday_assets, idx, variant);
            out.push(Candidate {
                candidate_id: format!("seed-int-{idx:02}a{variant}"),
                strategy: "intraday-drift".to_string(),
                args: vec![
                    "--ticker".to_string(),
                    ticker.to_string(),
                    "--no-plots".to_string(),
                ],
                rationale: format!("Web-evolved intraday mutant: {title}"),
                source: source.clone(),
                focus_assets: vec![ticker.to_string()],
                target_horizon: "1d".to_string(),
                min_observations: 20,
                min_signals: 8,
                is_diagnostic: false,
                is_seeded: true,
            });
        }
    }

    if tags.contains("breakout") && !tags.contains("intraday") {
        let mut args = vec![
            "--signal-mode".to_string(),
            "overbought".to_string(),
            "--threshold".to_string(),
            (50 + idx % 20).to_string(),
            "--assets".to_string(),
            "SPY".to_string(),
            "QQQ".to_string(),
        ];
        push_horizon(&mut args, HORIZON_1W);
        out.push(Candidate {
            candidate_id: format!("seed-bot-{idx:02}"),
            strategy: "ndx100-breadth-washout".to_string(),
            args,
            rationale: format!("Web-evolved breakout mutant: {title}"),
            source: source.clone(),
            focus_assets: vec!["SPY".to_string(), "QQQ".to_string()],
            target_horizon: HORIZON_1W.to_string(),
            min_observations: 16,
            min_signals: 6,
            is_diagnostic: false,
            is_seeded: true,
        });
    }

    if out.is_empty() {
        let mut args = vec![
            "--short-period".to_string(),
            "12".to_string(),
            "--long-period".to_string(),
            "100".to_string(),
            "--threshold".to_string(),
            "15".to_string(),
            "--universe".to_string(),
            "ndx100".to_string(),
            "--assets".to_string(),
            "SPY".to_string(),
            "SPXL".to_string(),
        ];
        push_horizon(&mut args, HORIZON_1M);
        out.push(Candidate {
            candidate_id: format!("seed-generic-{idx:02}"),
            strategy: "breadth-dual-ma".to_string(),
            args,
            rationale: format!("Web fallback mutant: {title}"),
            source: source,
            focus_assets: vec!["SPY".to_string(), "SPXL".to_string()],
            target_horizon: HORIZON_1M.to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
            is_seeded: true,
        });
    }

    out
}

fn build_seed_candidate_pool(
    seed_ideas: &[ExaSeed],
    min_candidates: usize,
    known_legacy: &HashSet<String>,
) -> Vec<Candidate> {
    let mut pool = Vec::new();
    let mut seen = HashSet::new();
    for (i, seed) in seed_ideas.iter().enumerate() {
        let base = build_seed_candidates(seed, i + 1);
        for (variant, cand) in base.iter().enumerate() {
            let mut placed = false;
            for delta in 0..6 {
                let to_try = emit_seed_mutation(cand, i + 1, variant + delta);
                if let Some(candidate) = to_try {
                    if insert_if_novel(&mut pool, &mut seen, known_legacy, candidate) {
                        placed = true;
                        break;
                    }
                }
            }
            if !placed && pool.len() >= min_candidates {
                break;
            }
        }
        if pool.len() >= min_candidates {
            break;
        }
    }

    if pool.len() < min_candidates {
        let seed_pool = pool.clone();
        let mut attempts = 0usize;
        while pool.len() < min_candidates && !seed_pool.is_empty() && attempts < min_candidates.saturating_mul(3) {
            let src = &seed_pool[attempts % seed_pool.len()];
            let mut inserted = false;
            for delta in 0..6 {
                if let Some(mutant) = emit_seed_mutation(src, attempts + 5000, attempts + delta) {
                    if insert_if_novel(&mut pool, &mut seen, known_legacy, mutant) {
                        inserted = true;
                        break;
                    }
                }
            }
            if !inserted {
                // If this source cannot produce a novel candidate, move on.
                if attempts > seed_pool.len() * 3 {
                    break;
                }
            }
            attempts += 1;
        }
    }
    pool
}

fn build_grid_candidate_pool() -> Vec<Candidate> {
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
                            candidate_id: format!(
                                "bw-{prefix}-{lookback}-{}{threshold}",
                                &mode[..3]
                            ),
                            strategy: "breadth-washout".to_string(),
                            args,
                            rationale:
                                "Test oversold/overbought breadth regime with short and medium MA windows."
                                    .to_string(),
                            source: "grid".to_string(),
                            focus_assets: vec!["SPY".to_string(), "SPXL".to_string()],
                            target_horizon: HORIZON_1M.to_string(),
                            min_observations: 16,
                            min_signals: 8,
                            is_diagnostic: false,
                            is_seeded: false,
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
                        target_horizon: HORIZON_1M.to_string(),
                        min_observations: 20,
                        min_signals: 10,
                        is_diagnostic: false,
                        is_seeded: false,
                    },
                );
            }
        }
    }

    for lookback in [8, 20, 50, 100] {
        for threshold in [50, 60, 70, 80] {
            for mode in ["oversold", "overbought"] {
                let mut args = vec![
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
                ];
                push_horizon(&mut args, HORIZON_1W);
                add_candidate(
                    &mut pool,
                    Candidate {
                        candidate_id: format!("bm-{lookback}-t{threshold}-{}", &mode[..3]),
                        strategy: "breadth-ma".to_string(),
                        args,
                        rationale:
                            "Single moving-average breadth trigger with threshold sweep and mode sweep."
                                .to_string(),
                        source: "grid".to_string(),
                        focus_assets: vec!["SPY".to_string(), "QQQ".to_string()],
                        target_horizon: HORIZON_1W.to_string(),
                        min_observations: 20,
                        min_signals: 8,
                        is_diagnostic: false,
                        is_seeded: false,
                    },
                );
            }
        }
    }

    for threshold in [55, 60, 65, 70] {
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

        add_candidate(
            &mut pool,
            Candidate {
                candidate_id: format!("ndx-wrap-{threshold}"),
                strategy: "ndx100-breadth-washout".to_string(),
                args,
                rationale:
                    "ndx100 wrapper candidate for alternative regime gate and universe settings."
                        .to_string(),
                source: "grid".to_string(),
                focus_assets: vec!["SPY".to_string(), "QQQ".to_string()],
                target_horizon: HORIZON_1W.to_string(),
                min_observations: 16,
                min_signals: 6,
                is_diagnostic: false,
                is_seeded: false,
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
            is_seeded: false,
        },
        Candidate {
            candidate_id: "overnight-vix".to_string(),
            strategy: "overnight-drift".to_string(),
            args: vec!["--no-plots".to_string()],
            rationale: "Execution baseline: built-in VIX filter should prefer safer regimes."
                .to_string(),
            source: "baseline".to_string(),
            focus_assets: vec!["SPY".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
            is_seeded: false,
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
            is_seeded: false,
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
            rationale: "Intraday long/short asymmetry probe on same execution structure."
                .to_string(),
            source: "baseline".to_string(),
            focus_assets: vec!["SPY".to_string()],
            target_horizon: "1d".to_string(),
            min_observations: 20,
            min_signals: 10,
            is_diagnostic: false,
            is_seeded: false,
        },
    ]);

    pool
}

fn build_candidate_pool(
    seed_ideas: &[ExaSeed],
    seed_web: bool,
    include_grid: bool,
    min_candidates: usize,
) -> Vec<Candidate> {
    let mut pool = Vec::new();
    let mut seen = HashSet::new();
    let known_legacy = build_known_signature_set();
    let seeded = if seed_web {
        build_seed_candidate_pool(seed_ideas, min_candidates, &known_legacy)
    } else {
        Vec::new()
    };
    let grid = if include_grid {
        build_grid_candidate_pool()
    } else {
        Vec::new()
    };

    if seed_web {
        if !seeded.is_empty() {
            for cand in seeded {
                seen.insert(signature_for_candidate(&cand));
                pool.push(cand);
            }
            if !include_grid && pool.len() < min_candidates {
                eprintln!(
                    "seed-web produced only {} novel candidates; running seed-only novel pool as-is (target {})",
                    pool.len(),
                    min_candidates
                );
            }
            if include_grid && pool.len() < min_candidates {
                let mut add_from_grid = grid;
                for cand in add_from_grid.drain(0..add_from_grid.len().min(min_candidates - pool.len()))
                {
                    let key = signature_for_candidate(&cand);
                    if seen.contains(&key) || known_legacy.contains(&key) {
                        continue;
                    }
                    seen.insert(key);
                    pool.push(cand);
                }
            }
        } else {
            if include_grid {
                eprintln!("seed web requested but no web seeds were produced; using grid fallback because --include-grid is enabled.");
                for cand in grid {
                    let key = signature_for_candidate(&cand);
                    if seen.contains(&key) || known_legacy.contains(&key) {
                        continue;
                    }
                    seen.insert(key);
                    pool.push(cand);
                }
            } else {
                eprintln!("seed web requested but no novel web candidates were generated.");
            }
        }
    } else {
        for cand in grid {
            let key = signature_for_candidate(&cand);
            if seen.insert(key) {
                pool.push(cand);
            }
        }
    }

    if seed_web && !include_grid && pool.is_empty() {
        return pool;
    }

    if pool.is_empty() {
        pool.extend(build_grid_candidate_pool());
    }

    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for cand in pool {
        let key = signature_for_candidate(&cand);
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
    let cum = safe_float(
        details
            .get("cumulative_return_pct")
            .or_else(|| details.get("cagr"))
            .unwrap_or(&Value::Null),
    )
    .unwrap_or(0.0);
    let sharpe = safe_float(details.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let dd = safe_float(
        details
            .get("max_drawdown_pct")
            .or_else(|| details.get("max_drawdown"))
            .unwrap_or(&Value::Null),
    )
    .unwrap_or(0.0);
    let var95 = safe_float(
        details
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

fn rank_rows(rows: &[CandidateReport], prioritize_seeded: bool) -> Vec<CandidateReport> {
    let mut ranked = rows.to_vec();
    ranked.sort_by(|a, b| {
        if prioritize_seeded && a.is_seeded != b.is_seeded {
            return b.is_seeded.cmp(&a.is_seeded);
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

fn profitability_blurb(
    category: &str,
    strategy: &str,
    train: &Value,
    test: &Value,
) -> String {
    if category != "Breadth" {
        let train_sharpe = safe_float(train.get("sharpe").or_else(|| train.get("cagr")).unwrap_or(&Value::Null))
            .unwrap_or(0.0);
        let test_sharpe = safe_float(test.get("sharpe").or_else(|| test.get("cagr")).unwrap_or(&Value::Null))
            .unwrap_or(0.0);
        let train_dd = safe_float(train.get("max_drawdown").or_else(|| train.get("max_drawdown_pct")).unwrap_or(&Value::Null))
            .unwrap_or(0.0);
        let test_dd = safe_float(test.get("max_drawdown").or_else(|| test.get("max_drawdown_pct")).unwrap_or(&Value::Null))
            .unwrap_or(0.0);
        let mut reasons = Vec::new();
        if train_sharpe > 0.2 && test_sharpe > 0.1 {
            reasons.push(format!(
                "{} (train/test) keeps positive risk-adjusted return ({:.3}, {:.3}) after session selection.",
                strategy, train_sharpe, test_sharpe
            ));
        } else if train_sharpe > 0.0 && test_sharpe > 0.0 {
            reasons.push(format!(
                "{} shows disciplined drift behavior with positive Sharpe on both windows ({:.3}, {:.3}).",
                strategy, train_sharpe, test_sharpe
            ));
        } else {
            reasons.push(format!(
                "{} has mixed intraday-like return profile; assess position sizing and timing filters.",
                strategy
            ));
        }
        if train_dd < 40.0 && test_dd < 50.0 {
            reasons.push(format!(
                "Drawdown profile appears controlled (train {:.2}%, test {:.2}%).",
                train_dd, test_dd
            ));
        } else {
            reasons.push(format!(
                "Large drawdowns (train {:.2}%, test {:.2}%) indicate leverage-aware sizing should be capped.",
                train_dd, test_dd
            ));
        }
        reasons.join(" ")
    } else {
        let train_sharpe = safe_float(train.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
        let test_sharpe = safe_float(test.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
        let train_dd = safe_float(train.get("max_drawdown_pct").unwrap_or(&Value::Null)).unwrap_or(0.0);
        let test_dd = safe_float(test.get("max_drawdown_pct").unwrap_or(&Value::Null)).unwrap_or(0.0);
        let train_obs = safe_u64(train.get("observations").unwrap_or(&Value::Null)).unwrap_or(0);
        let test_obs = safe_u64(test.get("observations").unwrap_or(&Value::Null)).unwrap_or(0);
        let mut reasons = Vec::new();
        if train_sharpe > 0.1 && test_sharpe > 0.1 {
            reasons.push(format!(
                "Strong breadth signal persistence: positive Sharpe in both train ({:.3}) and test ({:.3}) with {} and {} sample windows.",
                train_sharpe, test_sharpe, train_obs, test_obs
            ));
        } else if train_sharpe > 0.0 || test_sharpe > 0.0 {
            reasons.push(format!(
                "Cross-window asymmetry exists (train {:.3}, test {:.3}); regime-specific regime behavior may explain edge.",
                train_sharpe, test_sharpe
            ));
        } else {
            reasons.push("Breadth signals are noisy or timing-dependent in these windows; execution alpha is currently weak.".to_string());
        }
        if train_dd.abs() < 40.0 && test_dd.abs() < 45.0 {
            reasons.push(format!(
                "Risk profile is comparatively stable with max-drawdown near {}/{}%.",
                train_dd.abs(),
                test_dd.abs()
            ));
        } else {
            reasons.push(format!(
                "Higher max drawdown in one or both windows ({:.2}% / {:.2}%) suggests position and universe constraints.",
                train_dd.abs(),
                test_dd.abs()
            ));
        }
        reasons.join(" ")
    }
}

#[derive(Serialize)]
struct InteractiveRow {
    rank: usize,
    candidate_id: String,
    strategy: String,
    category: String,
    source: String,
    rationale: String,
    focus_assets: String,
    target_horizon: String,
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
    let ranked = rows.to_vec();
    let top_count = ranked.len().min(10);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let payload_rows: Vec<InteractiveRow> = ranked
        .iter()
        .enumerate()
        .map(|(idx, row)| InteractiveRow {
            rank: idx + 1,
            candidate_id: row.candidate_id.clone(),
            strategy: row.strategy.clone(),
            category: row.category.clone(),
            source: row.source.clone(),
            rationale: row.rationale.clone(),
            focus_assets: row.focus_assets.join(", "),
            target_horizon: row.target_horizon.clone(),
            args: row.args.clone(),
            combined_score: row.combined_score,
            train_score: row.train_score,
            test_score: row.test_score,
            train_details: row.train_details.clone(),
            test_details: row.test_details.clone(),
            why: profitability_blurb(
                &row.category,
                &row.strategy,
                &row.train_details,
                &row.test_details,
            ),
            is_seeded: row.is_seeded,
        })
        .collect();

    let row_json = serde_json::to_string_pretty(&payload_rows)
        .unwrap_or_else(|_| "[]".to_string());
    let top_seeded = ranked.iter().filter(|r| r.is_seeded).count();

    let state_json = serde_json::to_string_pretty(&json!({
        "generated_at": Utc::now().to_rfc3339(),
        "total_candidates": ranked.len(),
        "top_count": top_count,
        "seeded_in_top_10": ranked.iter().take(top_count).filter(|r| r.is_seeded).count(),
        "seeded_total": top_seeded,
    }))
    .unwrap_or_else(|_| "{}".to_string());

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Doob Autoresearch Loop - Top Candidates</title>
  <style>
    :root {{ --bg: #071023; --panel: #0f1f3a; --muted: #9db2cf; --text: #ecf4ff; --ok: #4ade80; --warn: #f59e0b; --bad: #f87171; }}
    body {{ margin: 0; font-family: Arial, Helvetica, sans-serif; background: radial-gradient(circle at 20% 10%, #13274a 0%, var(--bg) 50%, #020915 100%); color: var(--text); }}
    .container {{ width: min(1320px, 94vw); margin: 14px auto; }}
    .summary {{ display: grid; grid-template-columns: repeat(3,minmax(0,1fr)); gap: 10px; }}
    .card {{ background: linear-gradient(180deg,#102548,#0f1c39); border: 1px solid #21406e; border-radius: 10px; padding: 10px; }}
    .kpi {{ font-size: 1.4rem; font-weight: 700; }}
    .muted {{ color: var(--muted); }}
    .controls {{ margin: 10px 0; display: grid; grid-template-columns: repeat(auto-fit,minmax(140px,1fr)); gap: 8px; }}
    .panel {{ margin-top: 10px; border: 1px solid #244a79; background: rgba(14, 30, 56, 0.9); border-radius: 10px; padding: 10px; }}
    table {{ width: 100%; border-collapse: collapse; }}
    th, td {{ text-align: left; padding: 8px; border-bottom: 1px solid #233c63; font-size: 13px; }}
    th {{ background: #17325a; color: #d6e5ff; position: sticky; top: 0; }}
    .pill {{ display:inline-block; border:1px solid #365b8e; border-radius:999px; padding: 2px 8px; margin-right: 6px; font-size: 11px; }}
    .good {{ color: var(--ok); }}
    .bad {{ color: var(--bad); }}
    .muted-small {{ color: var(--muted); font-size: 11px; }}
    #details {{ margin-top: 12px; }}
    details {{ border: 1px solid #27508a; border-radius: 8px; background: #0f2450; padding: 6px; margin-top: 6px; }}
  </style>
</head>
<body>
<div class="container">
  <h2>Doob Autoresearch Loop — Top Candidates</h2>
  <div class="muted-small">Source: seed-web based candidate generation, seed-first ranking, run+test walk-forward windows.</div>
  <div class="summary">
    <div class="card"><div class="muted">Candidates passed</div><div class="kpi">{total}</div></div>
    <div class="card"><div class="muted">Top seeds shown</div><div class="kpi">{top_count}</div></div>
    <div class="card"><div class="muted">Seeded strategies in top 10</div><div class="kpi">{seeded_top}</div></div>
  </div>
  <div class="panel">
    <div class="controls">
      <input id="search" placeholder="Search candidate, strategy, rationale, source, asset" style="padding:6px; border-radius:8px; border:1px solid #2f5482; background:#0e2346; color:var(--text)" />
      <select id="cat" style="padding:6px; border-radius:8px; border:1px solid #2f5482; background:#0e2346; color:var(--text)"></select>
      <select id="src" style="padding:6px; border-radius:8px; border:1px solid #2f5482; background:#0e2346; color:var(--text)"></select>
      <select id="seeded" style="padding:6px; border-radius:8px; border:1px solid #2f5482; background:#0e2346; color:var(--text)">
        <option value="All">All</option>
        <option value="Seeded">Seeded only</option>
        <option value="Legacy">Legacy only</option>
      </select>
    </div>
    <div style="overflow:auto; max-height: 68vh;">
      <table id="table">
        <thead>
          <tr><th>Rank</th><th>Candidate</th><th>Category</th><th>Strategy</th><th>Score</th><th>Train</th><th>Test</th><th>Source</th><th>Why profitable?</th></tr>
        </thead>
        <tbody id="body"></tbody>
      </table>
    </div>
    <div id="details"></div>
  </div>
  <div class="muted-small">Generated: {generated}</div>
</div>
<script>
const rows = {rows_json};
const state = {state_json};
const sortBy = document.createElement('select');
const body = document.getElementById('body');
const details = document.getElementById('details');
const search = document.getElementById('search');
const cat = document.getElementById('cat');
const src = document.getElementById('src');
const seeded = document.getElementById('seeded');

function fmt(v) {{ return (typeof v === 'number' && Number.isFinite(v)) ? v.toLocaleString(undefined, {{ maximumFractionDigits: 3, minimumFractionDigits: 3 }}) : 'N/A'; }}
function pct(v) {{ return (typeof v === 'number' && Number.isFinite(v)) ? (v.toLocaleString(undefined, {{ maximumFractionDigits: 2, minimumFractionDigits: 2 }}) + '%') : 'N/A'; }}
function metric(v, k) {{
  if (v == null) return 'N/A';
  if (typeof v === 'number') {{
    if (['max_drawdown_pct', 'cumulative_return_pct', 'var_95', 'var_95_pct', 'cagr', 'sharpe', 'final_equity'].includes(k)) {{
      return fmt(v);
    }}
    return fmt(v);
  }}
  return String(v);
}}

function uniques(field) {{
  const values = rows.map(r => r[field]).filter((v, idx, arr) => arr.indexOf(v) === idx).sort();
  return ['All'].concat(values);
}}

function metricsHtml(row) {{
  const t = row.train_details || {{}};
  const s = row.test_details || {{}};
  const pick = (obj, a, b) => {{
    if (obj[a] !== undefined) return obj[a];
    if (obj[b] !== undefined) return obj[b];
    return null;
  }};
  const tSharpe = pick(t, 'sharpe', 'cagr');
  const sSharpe = pick(s, 'sharpe', 'cagr');
  const tDD = pick(t, 'max_drawdown_pct', 'max_drawdown');
  const sDD = pick(s, 'max_drawdown_pct', 'max_drawdown');
  const trainObs = t.observations || 0;
  const testObs = s.observations || 0;
  return 'Train Sharpe: ' + fmt(tSharpe) + ' / Test Sharpe: ' + fmt(sSharpe) +
         ' | Train DD: ' + pct(tDD) + ' / Test DD: ' + pct(sDD) +
         ' | Train obs: ' + trainObs + ' / Test obs: ' + testObs;
}}

function render() {{
  const q = search.value.toLowerCase();
  const catV = cat.value;
  const srcV = src.value;
  const seededV = seeded.value;
  const filtered = rows.filter(r => {{
    if (catV !== 'All' && r.category !== catV) return false;
    if (srcV !== 'All' && r.source !== srcV) return false;
    if (seededV === 'Seeded' && !r.is_seeded) return false;
    if (seededV === 'Legacy' && r.is_seeded) return false;
    if (!q) return true;
    const hay = (r.candidate_id + ' ' + r.strategy + ' ' + r.rationale + ' ' + r.focus_assets + ' ' + r.source + ' ' + r.category).toLowerCase();
    return hay.indexOf(q) !== -1;
  }});

  filtered.sort(function(a, b) {{
    if (b.is_seeded !== a.is_seeded) return b.is_seeded ? 1 : -1;
    if (b.combined_score !== a.combined_score) return b.combined_score - a.combined_score;
    return b.train_score - a.train_score;
  }});

  body.innerHTML = filtered.slice(0, 10).map((r, i) => {{
    const sourceType = r.source && r.source.indexOf('http') === 0 ? 'seed' : 'grid';
    const topTag = r.is_seeded ? '<span class=\"pill\">new seed</span>' : '<span class=\"pill\">existing</span>';
    const style = r.combined_score > 10000 ? 'good' : 'bad';
    return '<tr>' +
      '<td>' + (i + 1) + '</td>' +
      '<td><b>' + r.candidate_id + '</b> ' + topTag + '<div class=\"muted-small\">' + r.focus_assets + '</div></td>' +
      '<td><span class=\"pill\">' + r.category + '</span></td>' +
      '<td><span class=\"pill\">' + r.strategy + '</span></td>' +
      '<td class=\"' + style + '\">' + fmt(r.combined_score) + '</td>' +
      '<td>' + fmt(r.train_score) + '</td>' +
      '<td>' + fmt(r.test_score) + '</td>' +
      '<td>' + (sourceType === 'seed' ? r.source : r.source) + '</td>' +
      '<td>' + r.why + '</td>' +
      '</tr>' +
      '<tr><td colspan=\"9\"><details><summary>Full details</summary>' +
      '<div class=\"muted-small\">Args: <code>' + r.args.join(' ') + '</code></div>' +
      '<div class=\"muted-small\">Horizon: ' + r.target_horizon + ' | Train/Test Metrics: ' + metricsHtml(r) + '</div>' +
      '<p><strong>Profitable why:</strong> ' + r.why + '</p>' +
      '</details></td></tr>';
  }}).join('');

  details.innerHTML = filtered.slice(0, 10).map((r, i) => {{
    return '<details><summary><b>' + (i + 1) + '. ' + r.candidate_id + '</b> — ' + r.strategy + ' (' + r.category + ')</summary>' +
      '<p><span class=\"muted-small\">Source:</span> ' + r.source + ' | Horizon: ' + r.target_horizon + '</p>' +
      '<p><span class=\"muted-small\">Rationale:</span> ' + r.rationale + '</p>' +
      '<p><strong>Why this can be profitable:</strong> ' + r.why + '</p>' +
      '<p class=\"muted-small\">Train metric sample: ' + metric((r.train_details || {{}}).asset, 'asset') + ' / Test: ' + metric((r.test_details || {{}}).asset, 'asset') + '</p>' +
      '</details>';
  }}).join('');
}});

cat.innerHTML = uniques('category').map(v => '<option>' + v + '</option>').join('');
src.innerHTML = uniques('source').map(v => '<option>' + v + '</option>').join('');
[cat, src, seeded, search].forEach(el => el.addEventListener('input', render));
render();
</script>
</body>
</html>"#,
        rows_json = row_json,
        total = ranked.len(),
        top_count = top_count,
        seeded_top = ranked.iter().take(top_count).filter(|r| r.is_seeded).count(),
        generated = Utc::now().to_rfc3339(),
        state_json = state_json
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
            "is_seeded": row.is_seeded,
            "train_details": row.train_details.clone(),
            "test_details": row.test_details.clone(),
        });
        writeln!(f, "{}", serde_json::to_string(&line).unwrap_or_else(|_| "{}".to_string()))?;
    }
    Ok(())
}

fn print_top(rows: &[CandidateReport], k: usize, verbose: bool, prioritize_seeded: bool) {
    let rows = rank_rows(rows, prioritize_seeded);

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
            if row.is_seeded { "new".to_string() } else { "legacy".to_string() },
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
    let _ = dotenv();
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
        seed_ideas = fetch_exa_ideas(SEED_QUERIES, SEED_RESULTS_PER_QUERY);
        if args.verbose {
            println!("fetched {} web seeds", seed_ideas.len());
            println!(
                "novelty mode: web seeds must not match legacy strategy+argument signatures from baseline/grid."
            );
            if !args.include_grid {
                println!("running web-seed strategies only (plus novelty mutants).");
            } else {
                println!("including grid candidates in seed-web pool.");
            }
        }
        let report_path = Path::new("reports/autoresearch-exa-ideas.json");
        if let Err(err) = save_seed_ideas(report_path, &seed_ideas) {
            eprintln!("Failed to write exa ideas file: {err}");
        }
    }

    let mut candidates = build_candidate_pool(
        &seed_ideas,
        args.seed_web,
        args.include_grid,
        args.candidates,
    );
    if candidates.is_empty() && args.seed_web && !args.include_grid {
        eprintln!("No seed-web candidates were produced. Check EXA_API_KEY and API access, or run with --include-grid.");
        return;
    }
    if args.verbose {
        println!("candidate pool: {} total", candidates.len());
    }
    shuffle_candidates(&mut candidates, args.random_seed);
    if args.verbose {
        println!("candidate pool shuffled with seed {}", args.random_seed);
    }

    let mut seeded = Vec::new();
    let mut legacy = Vec::new();
    for candidate in candidates.into_iter() {
        if candidate.is_seeded {
            seeded.push(candidate);
        } else {
            legacy.push(candidate);
        }
    }
    shuffle_candidates(&mut seeded, args.random_seed ^ 0xA5A5A5A5_01020304);
    shuffle_candidates(&mut legacy, args.random_seed ^ 0x5A5A5A5A_FF00FF00);
    let mut candidates_to_run = Vec::new();
    if args.seed_web {
        candidates_to_run.extend(seeded);
        candidates_to_run.extend(legacy);
    } else {
        candidates_to_run.extend(legacy);
    }
    let total_candidates = candidates_to_run.len().min(args.candidates);
    if args.verbose {
        let seeded_count = candidates_to_run.iter().filter(|r| r.is_seeded).count();
        println!(
            "candidates selected for execution: {total_candidates} (seeded {seeded_count})"
        );
    }

    let mut results = Vec::new();
    for (idx, candidate) in candidates_to_run.iter().take(total_candidates).enumerate() {
        if args.verbose {
            println!(
                "evaluating candidate {} of {}",
                idx + 1,
                total_candidates
            );
        }
        if let Some(report) = evaluate_candidate(
            candidate,
            idx + 1,
            total_candidates,
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
    let ranked = rank_rows(&results, args.seed_web);
    let top_k = args.top.min(ranked.len());
    print_top(&ranked, top_k, args.verbose, args.seed_web);
    let report_rows = ranked.iter().take(10).cloned().collect::<Vec<_>>();
    let report_path = Path::new("reports/autoresearch-top10-interactive-report.html");
    if let Err(err) = save_interactive_report(
        report_path,
        &report_rows,
    ) {
        eprintln!("Failed to write interactive report: {err}");
    }
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

    let best = &ranked[0];
    print_best(best);
    let _ = Command::new("open").arg(report_path).spawn();
}
