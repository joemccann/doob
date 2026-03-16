use chrono::{Datelike, Duration, NaiveDate, Utc};
use clap::Parser;
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};
use dotenvy::dotenv;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
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
const RESEARCH_ASSETS: &[&str] = &["SPY", "QQQ", "SPXL", "IWM", "TQQQ"];

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

#[derive(Clone)]
struct RecordedResult {
    round: Option<u32>,
    signature: String,
    report: CandidateReport,
}

struct RoundSummary {
    round: u32,
    generated: usize,
    evaluated: usize,
    passed: usize,
    round_best: Option<f64>,
    global_best: Option<f64>,
    rel_improvement: f64,
}

struct LoopState {
    #[allow(dead_code)]
    round: u32,
    all_evaluated: HashSet<String>,
    exhausted_centers: HashSet<String>,
    recorded_results: Vec<RecordedResult>,
    round_summaries: Vec<RoundSummary>,
}

impl LoopState {
    fn new() -> Self {
        Self {
            round: 0,
            all_evaluated: HashSet::new(),
            exhausted_centers: HashSet::new(),
            recorded_results: Vec::new(),
            round_summaries: Vec::new(),
        }
    }

    fn current_best_score(&self) -> Option<f64> {
        self.recorded_results
            .iter()
            .map(|r| r.report.combined_score)
            .fold(None, |acc, score| match acc {
                None => Some(score),
                Some(best) => Some(if score > best { score } else { best }),
            })
    }

    fn retain_novel_candidates(&mut self, candidates: Vec<Candidate>) -> Vec<Candidate> {
        candidates
            .into_iter()
            .filter(|c| {
                let sig = param_signature(c);
                self.all_evaluated.insert(sig)
            })
            .collect()
    }

    fn record_round(
        &mut self,
        round: u32,
        generated: usize,
        evaluated: usize,
        round_reports: Vec<CandidateReport>,
        legacy_mode: bool,
    ) {
        let passed = round_reports.len();
        let round_best = round_reports
            .iter()
            .map(|r| r.combined_score)
            .fold(None::<f64>, |acc, s| Some(acc.map_or(s, |a: f64| a.max(s))));

        for report in round_reports {
            let sig = param_signature_from_args(&report.args);
            self.recorded_results.push(RecordedResult {
                round: if legacy_mode { None } else { Some(round) },
                signature: sig,
                report,
            });
        }

        let global_best = self.current_best_score();
        let prev_best = if self.round_summaries.is_empty() {
            None
        } else {
            self.round_summaries.last().and_then(|s| s.global_best)
        };

        let rel_improvement = match (prev_best, global_best) {
            (Some(prev), Some(curr)) => (curr - prev) / prev.abs().max(1.0),
            _ => 0.0,
        };

        self.round_summaries.push(RoundSummary {
            round,
            generated,
            evaluated,
            passed,
            round_best,
            global_best,
            rel_improvement,
        });
        self.round = round;
    }

    fn mark_center_exhausted(&mut self, signature: &str) {
        self.exhausted_centers.insert(signature.to_string());
    }

    fn is_center_exhausted(&self, signature: &str) -> bool {
        self.exhausted_centers.contains(signature)
    }
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

    /// Maximum refinement rounds (default: 10). Looping is the new default; use --no-loop for legacy single-pass
    #[arg(long, default_value_t = 10)]
    max_rounds: u32,

    /// Rounds without meaningful improvement before stopping (default: 3)
    #[arg(long, default_value_t = 3)]
    patience: u32,

    /// Minimum relative improvement to reset patience counter (default: 0.02)
    #[arg(long, default_value_t = 0.02)]
    min_improvement: f64,

    /// Number of top winners to refine each round (default: 5)
    #[arg(long, default_value_t = 5)]
    refine_top: usize,

    /// Maximum refinement variants per winner (default: 20)
    #[arg(long, default_value_t = 20)]
    refine_variants: usize,

    /// Disable iterative refinement (single-pass legacy mode)
    #[arg(long)]
    no_loop: bool,

    /// Disable evaluation cache (re-evaluate all candidates from scratch)
    #[arg(long)]
    no_cache: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct CachedEval {
    eval_key: String,
    passed: bool,
    train_score: f64,
    test_score: f64,
    combined_score: f64,
    train_details: Value,
    test_details: Value,
}

const EVAL_CACHE_PATH: &str = "reports/autoresearch-eval-cache.jsonl";

fn eval_cache_key(
    signature: &str,
    train_start: &str,
    train_end: &str,
    test_start: &str,
    test_end: &str,
) -> String {
    format!(
        "{sig}|{ts}|{te}|{xs}|{xe}",
        sig = signature,
        ts = train_start,
        te = train_end,
        xs = test_start,
        xe = test_end
    )
}

fn load_eval_cache(path: &Path) -> HashMap<String, CachedEval> {
    let mut cache = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return cache;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<CachedEval>(line) {
            cache.insert(entry.eval_key.clone(), entry);
        }
    }
    cache
}

fn append_eval_cache(path: &Path, entry: &CachedEval) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = File::options().append(true).create(true).open(path) {
        let _ = writeln!(
            file,
            "{}",
            serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string())
        );
    }
}

fn cached_evaluate_candidate(
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
    cache: &HashMap<String, CachedEval>,
    cache_path: &Path,
) -> Option<CandidateReport> {
    let sig = param_signature(candidate);
    let key = eval_cache_key(&sig, train_start, train_end, test_start, test_end);

    if let Some(hit) = cache.get(&key) {
        if verbose {
            let status = if hit.passed { "PASS" } else { "SKIP" };
            println!(
                "  [cache {status}] candidate {}/{}: {} (score={:.3})",
                idx, total, candidate.candidate_id, hit.combined_score
            );
        }
        if !hit.passed {
            return None;
        }
        return Some(CandidateReport {
            candidate_id: candidate.candidate_id.clone(),
            strategy: candidate.strategy.clone(),
            category: strategy_category(&candidate.strategy).to_string(),
            args: candidate.args.clone(),
            rule: candidate.rule.clone(),
            rationale: candidate.rationale.clone(),
            source: candidate.source.clone(),
            focus_asset: candidate.focus_asset.clone(),
            train_score: hit.train_score,
            test_score: hit.test_score,
            combined_score: hit.combined_score,
            train_details: hit.train_details.clone(),
            test_details: hit.test_details.clone(),
            is_seeded: candidate.is_seeded,
        });
    }

    // Cache miss — evaluate normally
    let result = evaluate_candidate(
        candidate,
        idx,
        total,
        doob_bin,
        train_start,
        train_end,
        test_start,
        test_end,
        train_sessions,
        test_sessions,
        verbose,
    );

    // Write to cache
    let entry = match &result {
        Some(report) => CachedEval {
            eval_key: key,
            passed: true,
            train_score: report.train_score,
            test_score: report.test_score,
            combined_score: report.combined_score,
            train_details: report.train_details.clone(),
            test_details: report.test_details.clone(),
        },
        None => CachedEval {
            eval_key: key,
            passed: false,
            train_score: 0.0,
            test_score: 0.0,
            combined_score: 0.0,
            train_details: json!({}),
            test_details: json!({}),
        },
    };
    append_eval_cache(cache_path, &entry);

    result
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

fn investment_case(rule: &str, asset: &str) -> String {
    match rule {
        RULE_TREND_MOMENTUM => format!(
            "Trend-following strategies on {asset} exploit the empirical tendency for asset prices to persist in directional moves. \
            By requiring alignment between short- and long-term moving averages, the strategy filters out choppy markets and enters \
            only when momentum is confirmed. This approach is favored by systematic traders seeking to capture the bulk of sustained \
            rallies while avoiding whipsaw losses during sideways periods."
        ),
        RULE_TREND_PULLBACK => format!(
            "Pullback entries on {asset} target temporary dips within an established uptrend — buying the short-term weakness while \
            the longer-term trend remains intact. This is a core institutional technique: it offers better risk/reward than chasing \
            breakouts because the entry price is lower and the stop is tighter. The strategy assumes that the dominant trend will \
            resume after a brief correction, a pattern well-documented in equity markets."
        ),
        RULE_RSI_REVERSION => format!(
            "Mean-reversion via RSI on {asset} exploits the tendency for oversold conditions to precede short-term bounces. When \
            RSI drops to extreme lows, selling pressure is typically exhausted and a reflexive recovery follows. This strategy is \
            particularly effective in liquid, large-cap instruments where panic selling creates predictable snap-back opportunities. \
            It provides diversification from trend-following approaches by profiting from the opposite market dynamic."
        ),
        RULE_VOL_REGIME => format!(
            "Volatility-regime filtering on {asset} restricts exposure to periods of low realized volatility, avoiding the fat-tailed \
            losses that occur during high-volatility regimes. The premise is that returns are more favorable and predictable in calm \
            markets. By stepping aside when volatility spikes, the strategy sacrifices some upside participation in exchange for \
            dramatically reduced drawdowns — a trade-off valued by risk-conscious portfolio managers."
        ),
        _ => format!(
            "This paper-research candidate on {asset} applies an adaptive signal derived from academic research to identify \
            favorable entry and exit conditions. The strategy is designed to exploit empirically-documented market patterns \
            with systematic, rules-based execution."
        ),
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

fn param_signature_from_args(args: &[String]) -> String {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--hypothesis-id" {
            i += 2; // skip flag and value
            continue;
        }
        if key.starts_with("--") {
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                let val = &args[i + 1];
                let normalized = if let Ok(f) = val.parse::<f64>() {
                    format!("{:.2}", f)
                } else {
                    val.clone()
                };
                pairs.push((key.clone(), normalized));
                i += 2;
            } else {
                pairs.push((key.clone(), String::new()));
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn param_signature(candidate: &Candidate) -> String {
    param_signature_from_args(&candidate.args)
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
        let parsed = parsed.unwrap_or(ExaResponse {
            results: Vec::new(),
        });

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
    let assets = RESEARCH_ASSETS;
    let mut out = Vec::new();

    let include_momentum = tags.contains("momentum") || tags.contains("intraday");
    let include_reversion = tags.contains("reversion");
    let include_regime = tags.contains("regime");

    let momentum_rules = if include_momentum {
        vec![RULE_TREND_MOMENTUM, RULE_TREND_PULLBACK]
    } else {
        vec![]
    };
    let reversion_rules = if include_reversion {
        vec![RULE_RSI_REVERSION]
    } else {
        vec![]
    };
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
    let assets = RESEARCH_ASSETS;
    let mut id = 0usize;

    for rule in [
        RULE_TREND_MOMENTUM,
        RULE_TREND_PULLBACK,
        RULE_RSI_REVERSION,
        RULE_VOL_REGIME,
    ] {
        for asset_idx in 0..assets.len() {
            for i in 0..FAST_WINDOW_SET.len() {
                for j in 0..SLOW_WINDOW_SET.len() {
                    let fast = FAST_WINDOW_SET[i];
                    let slow = SLOW_WINDOW_SET[j];
                    if (rule == RULE_TREND_MOMENTUM || rule == RULE_TREND_PULLBACK) && slow <= fast
                    {
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
                            args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
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
                            args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
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
        return build_deterministic_grid_candidates(
            min_candidates.max(MIN_CANDIDATES_TARGET_DEFAULT),
        );
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
    estimate_sessions(start_date, end_date)
        .unwrap_or(fallback)
        .max(1)
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

fn profitability_blurb(
    train: &Value,
    test: &Value,
    strategy: &str,
    rule: &str,
    args: &[String],
    asset: &str,
) -> String {
    let train_cagr = safe_float(train.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let test_cagr = safe_float(test.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let train_sharpe = safe_float(train.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let test_sharpe = safe_float(test.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let train_dd = safe_float(train.get("max_drawdown").unwrap_or(&Value::Null))
        .unwrap_or(0.0)
        .abs();
    let test_dd = safe_float(test.get("max_drawdown").unwrap_or(&Value::Null))
        .unwrap_or(0.0)
        .abs();
    let signal = rule_description(rule, args, asset);
    let mechanics = format!(
        "Why it works: {} This strategy is run through `{}` in `doob`.",
        signal, strategy
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
        println!(
            "  strategy: {} | rule: {}",
            candidate.strategy, candidate.rule
        );
        println!(
            "  asset: {} | source: {}",
            candidate.focus_asset, candidate.source
        );
        println!("  rationale: {}", candidate.rationale);
        println!("  args: {}", candidate.args.join(" "));
        println!(
            "  train window: {} -> {} (sessions={})",
            train_start, train_end, train_sessions
        );
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
        println!(
            "  train summary: {}",
            format_detail_summary(&train_run.details)
        );
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
        println!(
            "  test summary: {}",
            format_detail_summary(&test_run.details)
        );
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
    rationale: String,
    strategy_description: String,
    investment_rationale: String,
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
            rationale: row.rationale.clone(),
            strategy_description: rule_description(&row.rule, &row.args, &row.focus_asset),
            investment_rationale: investment_case(&row.rule, &row.focus_asset),
        })
        .collect();

    let rows_json = serde_json::to_string_pretty(&top_rows).unwrap_or_else(|_| "[]".to_string());
    let generated_date = Utc::now().format("%Y-%m-%d").to_string();
    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>doob Autoresearch Loop — Top {top_count} Research Strategies</title>
  <link rel="preconnect" href="https://fonts.googleapis.com" />
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
  <link href="https://fonts.googleapis.com/css2?family=DM+Mono:wght@300;400;500&display=swap" rel="stylesheet" />
  <style>
    :root {{
      --doob-bg:             #ffffff;
      --doob-bg-alt:         #f5f5f5;
      --doob-bg-surface:     #c7cdc8;
      --doob-text:           #1e1e1e;
      --doob-text-muted:     #4a5760;
      --doob-teal:           #3e5b63;
      --doob-teal-deep:      #2e454c;
      --doob-lime:           #c6e758;
      --doob-lime-hover:     #d4f06a;
      --doob-sky:            #5fc4e3;
      --doob-slate:          #4a5760;
      --doob-sage:           #c7cdc8;
      --doob-dark-bg:        #1e1e1e;
      --doob-dark-surface:   #2a3a42;
      --doob-dark-panel:     #3e5b63;
      --doob-dark-text:      #f5f5f5;
      --doob-dark-muted:     #c7cdc8;
      --doob-positive:       #c6e758;
      --doob-positive-text:  #3a5200;
      --doob-warning:        #f5a623;
      --doob-warning-text:   #7a4a00;
      --doob-negative:       #e85d5d;
      --doob-negative-text:  #7a1a1a;
      --doob-info:           #5fc4e3;
      --doob-info-text:      #1a5a6a;
      --doob-font-display:   "Helvetica Now Display", -apple-system, BlinkMacSystemFont,
                              "Avenir Next", Avenir, "Segoe UI", "Helvetica Neue",
                              Helvetica, Cantarell, Ubuntu, Roboto, Noto, Arial, sans-serif;
      --doob-font-mono:      "DM Mono", Menlo, Consolas, Monaco, "Liberation Mono",
                              "Lucida Console", monospace;
      --doob-radius-sm:      6px;
      --doob-radius-md:      10px;
      --doob-radius-lg:      16px;
      --doob-radius-xl:      24px;
      --doob-shadow-sm:      0 1px 2px rgba(30,30,30,0.06);
      --doob-shadow-md:      0 4px 12px rgba(30,30,30,0.08);
      --doob-shadow-lg:      0 8px 32px rgba(30,30,30,0.12);
      --doob-ease:           cubic-bezier(0.25, 0.1, 0.25, 1);
      --doob-duration:       200ms;
    }}
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: var(--doob-font-display);
      background: var(--doob-bg);
      color: var(--doob-text);
      line-height: 1.5;
      -webkit-font-smoothing: antialiased;
    }}
    .report-header {{
      background: var(--doob-teal);
      color: #fff;
      padding: 48px clamp(20px, 4vw, 40px) 40px;
      border-radius: 0 0 var(--doob-radius-xl) var(--doob-radius-xl);
    }}
    .report-header-inner {{
      max-width: 1540px;
      margin: 0 auto;
      display: flex;
      justify-content: space-between;
      align-items: flex-start;
      gap: 32px;
      flex-wrap: wrap;
    }}
    .report-brand {{
      font-size: 28px;
      letter-spacing: -0.02em;
      font-weight: 400;
    }}
    .report-brand span {{ color: var(--doob-lime); }}
    .report-title {{
      font-size: clamp(32px, 5vw, 54px);
      font-weight: 400;
      letter-spacing: -0.02em;
      line-height: 1.05;
      margin-top: 16px;
      max-width: 700px;
    }}
    .report-subtitle {{
      font-family: var(--doob-font-mono);
      font-size: 13px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--doob-lime);
      margin-top: 8px;
    }}
    .report-meta {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      color: var(--doob-sage);
      margin-top: 20px;
      display: flex;
      gap: 24px;
      flex-wrap: wrap;
    }}
    .report-meta span {{ display: flex; align-items: center; gap: 6px; }}
    .container {{
      max-width: 1540px;
      margin: 0 auto;
      padding: 0 clamp(20px, 4vw, 40px);
    }}
    .kpi-bar {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
      gap: 12px;
      margin: 24px 0;
    }}
    .kpi {{
      background: var(--doob-bg-alt);
      border: 1px solid var(--doob-sage);
      border-radius: var(--doob-radius-md);
      padding: 16px;
    }}
    .kpi-label {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--doob-slate);
      margin-bottom: 4px;
    }}
    .kpi-value {{
      font-family: var(--doob-font-mono);
      font-size: 28px;
      font-weight: 400;
      letter-spacing: -0.02em;
    }}
    .kpi-value.positive {{ color: var(--doob-positive-text); }}
    .kpi-value.negative {{ color: var(--doob-negative-text); }}
    .controls {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
      gap: 10px;
      margin: 20px 0;
    }}
    .controls input, .controls select {{
      font-family: var(--doob-font-mono);
      font-size: 13px;
      padding: 10px 14px;
      border: 1px solid var(--doob-sage);
      border-radius: var(--doob-radius-sm);
      background: var(--doob-bg);
      color: var(--doob-text);
      outline: none;
      transition: border-color var(--doob-duration) var(--doob-ease);
    }}
    .controls input:focus, .controls select:focus {{
      border-color: var(--doob-teal);
      box-shadow: 0 0 0 3px rgba(62, 91, 99, 0.12);
    }}
    .controls input::placeholder {{ color: var(--doob-sage); }}
    .table-wrap {{
      background: var(--doob-bg);
      border: 1px solid var(--doob-sage);
      border-radius: var(--doob-radius-lg);
      overflow: hidden;
      box-shadow: var(--doob-shadow-md);
    }}
    .table-scroll {{ overflow: auto; max-height: 70vh; }}
    table {{ width: 100%; border-collapse: collapse; font-size: 13px; }}
    th {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      text-align: left;
      padding: 12px 14px;
      background: var(--doob-bg-alt);
      color: var(--doob-slate);
      border-bottom: 2px solid var(--doob-teal);
      position: sticky;
      top: 0;
      z-index: 1;
    }}
    th.sortable {{ cursor: pointer; user-select: none; }}
    th.sortable:hover {{ color: var(--doob-teal); }}
    th.sorted-asc::after {{ content: ' \u25B2'; font-size: 9px; }}
    th.sorted-desc::after {{ content: ' \u25BC'; font-size: 9px; }}
    td {{
      padding: 10px 14px;
      border-bottom: 1px solid rgba(199, 205, 200, 0.5);
      vertical-align: top;
    }}
    tr:last-child td {{ border-bottom: none; }}
    tr:hover td {{ background: rgba(62, 91, 99, 0.03); }}
    tr.expanded td {{ background: rgba(62, 91, 99, 0.05); }}
    td.mono {{ font-family: var(--doob-font-mono); }}
    td.pos {{ color: var(--doob-positive-text); }}
    td.neg {{ color: var(--doob-negative-text); }}
    td.rank {{
      font-family: var(--doob-font-mono);
      font-weight: 500;
      color: var(--doob-teal);
    }}
    .pill {{
      display: inline-flex;
      align-items: center;
      gap: 4px;
      font-family: var(--doob-font-mono);
      font-size: 11px;
      padding: 2px 10px;
      border-radius: 999px;
      letter-spacing: 0.04em;
      white-space: nowrap;
    }}
    .pill-teal {{ background: rgba(62,91,99,0.1); color: var(--doob-teal); }}
    .pill-lime {{ background: rgba(198,231,88,0.2); color: var(--doob-positive-text); }}
    .pill-sky  {{ background: rgba(95,196,227,0.15); color: var(--doob-info-text); }}
    .row-details {{
      background: var(--doob-bg-alt);
      border: 1px solid var(--doob-sage);
      border-radius: var(--doob-radius-md);
      padding: 14px 16px;
      margin-top: 8px;
      font-size: 12px;
      line-height: 1.6;
    }}
    .row-details .detail-grid {{
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 16px;
    }}
    .row-details h4 {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--doob-slate);
      margin-bottom: 8px;
      padding-bottom: 4px;
      border-bottom: 1px solid var(--doob-sage);
    }}
    .detail-row {{
      display: flex;
      justify-content: space-between;
      padding: 3px 0;
    }}
    .detail-row .label {{ color: var(--doob-text-muted); }}
    .detail-row .value {{ font-family: var(--doob-font-mono); font-weight: 500; }}
    .rationale {{
      margin-top: 12px;
      padding: 12px;
      background: rgba(62,91,99,0.05);
      border-radius: var(--doob-radius-sm);
      border-left: 3px solid var(--doob-teal);
      color: var(--doob-text-muted);
      font-size: 12px;
      line-height: 1.6;
    }}
    .source-link {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      color: var(--doob-info-text);
      text-decoration: underline;
      text-underline-offset: 2px;
    }}
    .info-bubble {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 16px;
      height: 16px;
      border-radius: 50%;
      background: rgba(95,196,227,0.15);
      color: var(--doob-info-text);
      font-family: var(--doob-font-mono);
      font-size: 10px;
      font-weight: 500;
      cursor: help;
      position: relative;
      margin-left: 6px;
      vertical-align: middle;
      flex-shrink: 0;
    }}
    .info-bubble .info-tip {{
      display: none;
      position: absolute;
      top: calc(100% + 8px);
      left: 0;
      width: 280px;
      padding: 12px 14px;
      background: var(--doob-teal-deep);
      color: #fff;
      border-radius: var(--doob-radius-sm);
      font-family: var(--doob-font-display);
      font-size: 12px;
      font-weight: 400;
      line-height: 1.5;
      letter-spacing: 0;
      text-transform: none;
      box-shadow: var(--doob-shadow-lg);
      z-index: 100;
    }}
    .info-bubble .info-tip::before {{
      content: '';
      position: absolute;
      bottom: 100%;
      left: 6px;
      border: 6px solid transparent;
      border-bottom-color: var(--doob-teal-deep);
    }}
    .info-bubble:hover .info-tip {{ display: block; }}
    .analysis-section {{
      margin-top: 16px;
      border: 1px solid var(--doob-sage);
      border-radius: var(--doob-radius-md);
      overflow: hidden;
    }}
    .analysis-header {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--doob-slate);
      padding: 10px 16px;
      background: var(--doob-bg-alt);
      border-bottom: 1px solid var(--doob-sage);
    }}
    .analysis-body {{
      padding: 16px;
    }}
    .analysis-block {{
      margin-bottom: 14px;
    }}
    .analysis-block:last-child {{ margin-bottom: 0; }}
    .analysis-block-label {{
      font-family: var(--doob-font-mono);
      font-size: 11px;
      letter-spacing: 0.06em;
      text-transform: uppercase;
      color: var(--doob-teal);
      font-weight: 500;
      margin-bottom: 4px;
    }}
    .analysis-block-text {{
      font-size: 13px;
      line-height: 1.6;
      color: var(--doob-text-muted);
    }}
    .analysis-block-text a {{
      color: var(--doob-info-text);
      text-decoration: underline;
      text-underline-offset: 2px;
    }}
    .toggle-btn {{
      background: none;
      border: 1px solid var(--doob-sage);
      border-radius: var(--doob-radius-sm);
      width: 28px;
      height: 28px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      cursor: pointer;
      font-size: 12px;
      color: var(--doob-slate);
      transition: all var(--doob-duration) var(--doob-ease);
    }}
    .toggle-btn:hover {{ border-color: var(--doob-teal); color: var(--doob-teal); }}
    .toggle-btn.open {{ background: var(--doob-teal); border-color: var(--doob-teal); color: #fff; }}
    .report-footer {{
      background: var(--doob-teal);
      color: #fff;
      padding: 32px clamp(20px, 4vw, 40px);
      border-radius: var(--doob-radius-xl) var(--doob-radius-xl) 0 0;
      margin-top: 64px;
    }}
    .report-footer-inner {{
      max-width: 1540px;
      margin: 0 auto;
      display: flex;
      justify-content: space-between;
      align-items: center;
      flex-wrap: wrap;
      gap: 16px;
    }}
    .report-footer .brand {{ font-size: 20px; letter-spacing: -0.02em; }}
    .report-footer .meta {{ font-family: var(--doob-font-mono); font-size: 11px; color: var(--doob-sage); }}
    @media (max-width: 768px) {{
      .report-header-inner {{ flex-direction: column; }}
      .kpi-bar {{ grid-template-columns: repeat(2, 1fr); }}
      .row-details .detail-grid {{ grid-template-columns: 1fr; }}
    }}
  </style>
</head>
<body>

<div class="report-header">
  <div class="report-header-inner">
    <div>
      <div class="report-brand">doob<span>.</span></div>
      <h1 class="report-title">Autoresearch Loop &mdash; Paper-Research Top {top_count}</h1>
      <div class="report-subtitle">Net-new strategy discovery from Exa/arXiv paper hypotheses</div>
      <div class="report-meta">
        <span>Generated: <strong>{generated_date}</strong></span>
        <span>Candidates: <strong>{total}</strong></span>
        <span>Seeded in top: <strong>{seeded_top}</strong></span>
      </div>
    </div>
  </div>
</div>

<div class="container" style="padding-top: 28px;">
  <div class="kpi-bar">
    <div class="kpi">
      <div class="kpi-label">Candidates Evaluated</div>
      <div class="kpi-value" id="kpi-candidates">&mdash;</div>
    </div>
    <div class="kpi">
      <div class="kpi-label">Top Shown</div>
      <div class="kpi-value" id="kpi-top">&mdash;</div>
    </div>
    <div class="kpi">
      <div class="kpi-label">Best CAGR (Train)</div>
      <div class="kpi-value positive" id="kpi-cagr">&mdash;</div>
    </div>
    <div class="kpi">
      <div class="kpi-label">Best Sharpe (Train)</div>
      <div class="kpi-value positive" id="kpi-sharpe">&mdash;</div>
    </div>
    <div class="kpi">
      <div class="kpi-label">Seeded in Top</div>
      <div class="kpi-value" id="kpi-seeded">&mdash;</div>
    </div>
  </div>

  <div class="controls">
    <input id="search" placeholder="Search strategy, rule, asset, source..." />
    <select id="cat">
      <option value="All">All Categories</option>
    </select>
    <select id="src">
      <option value="All">All Sources</option>
    </select>
    <select id="seeded">
      <option value="All">All Origins</option>
      <option value="Seeded">Seeded</option>
      <option value="Fallback">Fallback</option>
    </select>
  </div>

  <div class="table-wrap">
    <div class="table-scroll">
      <table>
        <thead>
          <tr>
            <th style="width:50px;">#</th>
            <th class="sortable" data-col="candidate_id">Candidate</th>
            <th class="sortable" data-col="rule">Rule</th>
            <th class="sortable" data-col="focus_asset">Asset</th>
            <th class="sortable" data-col="category">Category</th>
            <th class="sortable" data-col="combined_score">Score</th>
            <th class="sortable" data-col="train_cagr">Train CAGR</th>
            <th class="sortable" data-col="test_cagr">Test CAGR</th>
            <th>Source</th>
            <th style="width:36px;"></th>
          </tr>
        </thead>
        <tbody id="body"></tbody>
      </table>
    </div>
  </div>
  <div id="details"></div>
</div>

<div class="report-footer">
  <div class="report-footer-inner">
    <div class="brand">doob<span style="color:var(--doob-lime);">.</span></div>
    <div class="meta">Quantitative Strategy Research &middot; Autoresearch Report</div>
  </div>
</div>

<script>
const rows = {rows_json};
const tbody = document.getElementById('body');
const searchEl = document.getElementById('search');
const catEl = document.getElementById('cat');
const srcEl = document.getElementById('src');
const seededEl = document.getElementById('seeded');

const cats = [...new Set(rows.map(r => r.category))];
cats.forEach(c => {{ const o = document.createElement('option'); o.value = c; o.textContent = c; catEl.appendChild(o); }});
const srcs = [...new Set(rows.map(r => {{ try {{ return new URL(r.source).hostname; }} catch {{ return r.source; }} }}))];
srcs.forEach(s => {{ const o = document.createElement('option'); o.value = s; o.textContent = s; srcEl.appendChild(o); }});

document.getElementById('kpi-candidates').textContent = rows.length;
document.getElementById('kpi-top').textContent = rows.length;
if (rows.length) {{
  const bestCagr = Math.max(...rows.map(r => r.train_details?.cagr ?? 0));
  const bestSharpe = Math.max(...rows.map(r => r.train_details?.sharpe ?? 0));
  document.getElementById('kpi-cagr').textContent = (bestCagr * 100).toFixed(1) + '%';
  document.getElementById('kpi-sharpe').textContent = bestSharpe.toFixed(3);
  document.getElementById('kpi-seeded').textContent = rows.filter(r => r.is_seeded).length;
}}

function fmt(v, pct) {{
  if (v == null || v === undefined) return '\u2014';
  return pct ? (v * 100).toFixed(1) + '%' : v.toFixed(3);
}}
function cagrClass(v) {{
  if (v == null) return '';
  return v >= 0 ? 'pos' : 'neg';
}}

function renderRows(data) {{
  tbody.innerHTML = '';
  data.forEach(r => {{
    let srcHost, isUrl = false;
    try {{ const u = new URL(r.source); srcHost = u.hostname.replace('www.', ''); isUrl = true; }} catch {{ srcHost = r.source || '\u2014'; }}
    const srcCell = isUrl
      ? `<a class="source-link" href="${{r.source}}" target="_blank" rel="noopener">${{srcHost}}</a>`
      : `<span style="font-family:var(--doob-font-mono);font-size:11px;color:var(--doob-slate);">${{srcHost}}</span>`;

    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td class="rank">${{r.rank}}</td>
      <td style="font-size:12px;">${{r.candidate_id || '\u2014'}}</td>
      <td><span class="pill pill-sky">${{r.rule || '\u2014'}}</span></td>
      <td><span class="pill pill-teal">${{r.focus_asset || '\u2014'}}</span></td>
      <td>${{r.category || '\u2014'}}</td>
      <td class="mono" style="font-weight:500;">${{fmt(r.combined_score, false)}}</td>
      <td class="mono ${{cagrClass(r.train_details?.cagr)}}">${{fmt(r.train_details?.cagr, true)}}</td>
      <td class="mono ${{cagrClass(r.test_details?.cagr)}}">${{fmt(r.test_details?.cagr, true)}}</td>
      <td>${{srcCell}}</td>
      <td><button class="toggle-btn" data-rank="${{r.rank}}">+</button></td>
    `;
    tbody.appendChild(tr);

    const detailTr = document.createElement('tr');
    detailTr.id = 'detail-' + r.rank;
    detailTr.style.display = 'none';
    detailTr.innerHTML = `<td colspan="10">
      <div class="row-details">
        <div class="detail-grid">
          <div>
            <h4 style="display:flex;align-items:center;">Train Window<span class="info-bubble">i<span class="info-tip">The in-sample period (2020\u20132024) used to fit and calibrate the strategy. Strong train metrics confirm the signal exists historically, but high train scores alone can indicate overfitting. Always compare with the test window.</span></span></h4>
            <div class="detail-row"><span class="label">CAGR</span><span class="value ${{cagrClass(r.train_details?.cagr)}}">${{fmt(r.train_details?.cagr, true)}}</span></div>
            <div class="detail-row"><span class="label">Sharpe</span><span class="value">${{fmt(r.train_details?.sharpe, false)}}</span></div>
            <div class="detail-row"><span class="label">Max Drawdown</span><span class="value neg">${{fmt(r.train_details?.max_drawdown, true)}}</span></div>
            <div class="detail-row"><span class="label">Final Equity</span><span class="value">$${{(r.train_details?.final_equity || 0).toLocaleString('en-US', {{maximumFractionDigits: 0}})}}</span></div>
            <div class="detail-row"><span class="label">VaR 95</span><span class="value">${{fmt(r.train_details?.var_95, true)}}</span></div>
          </div>
          <div>
            <h4 style="display:flex;align-items:center;">Test Window<span class="info-bubble">i<span class="info-tip">The out-of-sample period (2025\u2013present) the strategy has never seen during calibration. Test performance is the most reliable indicator of real-world viability. Strategies that perform well in both windows are more likely to be robust rather than overfit.</span></span></h4>
            <div class="detail-row"><span class="label">CAGR</span><span class="value ${{cagrClass(r.test_details?.cagr)}}">${{fmt(r.test_details?.cagr, true)}}</span></div>
            <div class="detail-row"><span class="label">Sharpe</span><span class="value">${{fmt(r.test_details?.sharpe, false)}}</span></div>
            <div class="detail-row"><span class="label">Max Drawdown</span><span class="value neg">${{fmt(r.test_details?.max_drawdown, true)}}</span></div>
            <div class="detail-row"><span class="label">Final Equity</span><span class="value">$${{(r.test_details?.final_equity || 0).toLocaleString('en-US', {{maximumFractionDigits: 0}})}}</span></div>
            <div class="detail-row"><span class="label">VaR 95</span><span class="value">${{fmt(r.test_details?.var_95, true)}}</span></div>
          </div>
        </div>
        <div class="analysis-section">
          <div class="analysis-header">Analysis</div>
          <div class="analysis-body">
            <div class="analysis-block">
              <div class="analysis-block-label">Strategy</div>
              <div class="analysis-block-text">${{r.strategy_description || ''}}</div>
            </div>
            <div class="analysis-block">
              <div class="analysis-block-label">Research Basis</div>
              <div class="analysis-block-text">${{r.rationale || ''}}${{isUrl ? ` <a href="${{r.source}}" target="_blank" rel="noopener">View source \u2192</a>` : ''}}</div>
            </div>
            <div class="analysis-block">
              <div class="analysis-block-label">Investment Case</div>
              <div class="analysis-block-text">${{r.investment_rationale || ''}}</div>
            </div>
            <div class="analysis-block">
              <div class="analysis-block-label">Performance Summary</div>
              <div class="analysis-block-text">${{r.why || ''}}</div>
            </div>
          </div>
        </div>
      </div>
    </td>`;
    tbody.appendChild(detailTr);
  }});
}}

renderRows(rows);

function applyFilters() {{
  const q = searchEl.value.toLowerCase();
  const cat = catEl.value;
  const src = srcEl.value;
  const seed = seededEl.value;
  const filtered = rows.filter(r => {{
    if (q) {{
      const haystack = [r.candidate_id, r.rule, r.focus_asset, r.source, r.why, r.category].join(' ').toLowerCase();
      if (!haystack.includes(q)) return false;
    }}
    if (cat !== 'All' && r.category !== cat) return false;
    if (src !== 'All') {{
      let h; try {{ h = new URL(r.source).hostname; }} catch {{ h = r.source; }}
      if (h !== src) return false;
    }}
    if (seed === 'Seeded' && !r.is_seeded) return false;
    if (seed === 'Fallback' && r.is_seeded) return false;
    return true;
  }});
  renderRows(filtered);
}}

searchEl.addEventListener('input', applyFilters);
catEl.addEventListener('change', applyFilters);
srcEl.addEventListener('change', applyFilters);
seededEl.addEventListener('change', applyFilters);

document.addEventListener('click', e => {{
  const btn = e.target.closest('.toggle-btn');
  if (!btn) return;
  const rank = btn.dataset.rank;
  const row = document.getElementById('detail-' + rank);
  if (!row) return;
  const open = row.style.display !== 'none';
  row.style.display = open ? 'none' : '';
  btn.textContent = open ? '+' : '\u2212';
  btn.classList.toggle('open', !open);
  btn.closest('tr').classList.toggle('expanded', !open);
}});

let sortCol = null, sortDir = 'desc';
document.querySelectorAll('th.sortable').forEach(th => {{
  th.addEventListener('click', () => {{
    const col = th.dataset.col;
    if (sortCol === col) {{ sortDir = sortDir === 'asc' ? 'desc' : 'asc'; }}
    else {{ sortCol = col; sortDir = 'desc'; }}
    document.querySelectorAll('th.sortable').forEach(t => t.classList.remove('sorted-asc', 'sorted-desc'));
    th.classList.add(sortDir === 'asc' ? 'sorted-asc' : 'sorted-desc');
    rows.sort((a, b) => {{
      let va, vb;
      if (col === 'train_cagr') {{ va = a.train_details?.cagr ?? 0; vb = b.train_details?.cagr ?? 0; }}
      else if (col === 'test_cagr') {{ va = a.test_details?.cagr ?? 0; vb = b.test_details?.cagr ?? 0; }}
      else {{ va = a[col] ?? ''; vb = b[col] ?? ''; }}
      if (typeof va === 'string') {{ return sortDir === 'asc' ? va.localeCompare(vb) : vb.localeCompare(va); }}
      return sortDir === 'asc' ? va - vb : vb - va;
    }});
    rows.forEach((r, i) => r.rank = i + 1);
    applyFilters();
  }});
}});
</script>
</body>
</html>"#,
        rows_json = rows_json,
        total = rows.len(),
        top_count = rows.len().min(10),
        seeded_top = rows.iter().take(10).filter(|r| r.is_seeded).count(),
        generated_date = generated_date,
    );

    std::fs::write(path, html)
}

fn save_ledger(path: &Path, rows: &[RecordedResult]) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::options().append(true).create(true).open(path)?;
    let ts = Utc::now().to_rfc3339();
    for row in rows {
        let mut record = json!({
            "timestamp": ts,
            "candidate_id": row.report.candidate_id,
            "strategy": row.report.strategy,
            "category": row.report.category,
            "rule": row.report.rule,
            "args": row.report.args,
            "source": row.report.source,
            "focus_asset": row.report.focus_asset,
            "train_score": row.report.train_score,
            "test_score": row.report.test_score,
            "combined_score": row.report.combined_score,
            "is_seeded": row.report.is_seeded,
            "train_details": row.report.train_details,
            "test_details": row.report.test_details,
        });
        if let Some(round) = row.round {
            record["round"] = json!(round);
        }
        writeln!(
            file,
            "{}",
            serde_json::to_string(&record).unwrap_or_else(|_| "{}".to_string())
        )?;
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
        "rank", "score", "seeded", "category", "strategy", "rule", "source", "asset", "train",
        "test",
    ]);
    for (i, row) in rows.iter().take(k).enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            format!("{:.3}", row.combined_score),
            if row.is_seeded {
                "seed".to_string()
            } else {
                "fallback".to_string()
            },
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
    table.add_row(vec![
        "command".to_string(),
        format!(
            "doob --output json run {} {}",
            best.strategy,
            best.args.join(" ")
        ),
    ]);
    table.add_row(vec!["rationale".to_string(), best.rationale.clone()]);
    table.add_row(vec![
        "train details".to_string(),
        format_detail_summary(&best.train_details),
    ]);
    table.add_row(vec![
        "test details".to_string(),
        format_detail_summary(&best.test_details),
    ]);
    println!("{}", table);
}

fn has_converged(summaries: &[RoundSummary], patience: u32, min_improvement: f64) -> bool {
    if summaries.len() <= 1 {
        return false;
    }
    let mut stale_count = 0u32;
    for i in 1..summaries.len() {
        let prev_best = summaries[i - 1].global_best;
        let curr_best = summaries[i].global_best;
        let rel_imp = match (prev_best, curr_best) {
            (Some(prev), Some(curr)) => (curr - prev) / prev.abs().max(1.0),
            _ => 0.0,
        };
        if rel_imp < min_improvement {
            stale_count += 1;
        } else {
            stale_count = 0;
        }
    }
    stale_count >= patience
}

fn adjacent_values_u32(set: &[u32], current: u32) -> Vec<u32> {
    let mut result = Vec::new();
    let pos = set.iter().position(|&v| v == current);
    match pos {
        Some(idx) => {
            if idx > 0 {
                result.push(set[idx - 1]);
            }
            if idx + 1 < set.len() {
                result.push(set[idx + 1]);
            }
        }
        None => {
            let mut below = None;
            let mut above = None;
            for &v in set {
                if v < current {
                    below = Some(v);
                } else if v > current && above.is_none() {
                    above = Some(v);
                }
            }
            if let Some(b) = below {
                result.push(b);
            }
            if let Some(a) = above {
                result.push(a);
            }
        }
    }
    result
}

fn adjacent_values_f64(set: &[f64], current: f64) -> Vec<f64> {
    let mut result = Vec::new();
    let pos = set.iter().position(|&v| (v - current).abs() < 1e-6);
    match pos {
        Some(idx) => {
            if idx > 0 {
                result.push(set[idx - 1]);
            }
            if idx + 1 < set.len() {
                result.push(set[idx + 1]);
            }
        }
        None => {
            let mut below = None;
            let mut above = None;
            for &v in set {
                if v < current - 1e-6 {
                    below = Some(v);
                } else if v > current + 1e-6 && above.is_none() {
                    above = Some(v);
                }
            }
            if let Some(b) = below {
                result.push(b);
            }
            if let Some(a) = above {
                result.push(a);
            }
        }
    }
    result
}

fn make_refined_candidate(
    rule: &str,
    asset: &str,
    fast: u32,
    slow: u32,
    rsi_window: u32,
    rsi_oversold: u32,
    rsi_overbought: u32,
    vol_window: u32,
    vol_cap: f64,
    round: u32,
    winner_idx: usize,
    variant_idx: usize,
) -> Candidate {
    let candidate_id = format!("refine-R{round}-W{winner_idx:02}-V{variant_idx:03}");
    let args = vec![
        "--asset".to_string(),
        asset.to_string(),
        "--rule".to_string(),
        rule.to_string(),
        "--fast-window".to_string(),
        fast.to_string(),
        "--slow-window".to_string(),
        slow.to_string(),
        "--rsi-window".to_string(),
        rsi_window.to_string(),
        "--rsi-oversold".to_string(),
        rsi_oversold.to_string(),
        "--rsi-overbought".to_string(),
        rsi_overbought.to_string(),
        "--vol-window".to_string(),
        vol_window.to_string(),
        "--vol-cap".to_string(),
        format!("{:.2}", vol_cap),
        "--hypothesis-id".to_string(),
        candidate_id.clone(),
    ];
    Candidate {
        candidate_id,
        strategy: RESEARCH_STRATEGY.to_string(),
        rule: rule.to_string(),
        args,
        rationale: format!("Refinement variant (round {round}, winner {winner_idx})"),
        source: "refinement".to_string(),
        focus_asset: asset.to_string(),
        is_seeded: false,
        _min_signals: 10,
        _min_observations: 20,
    }
}

fn refine_around_winner(
    winner: &CandidateReport,
    loop_state: &LoopState,
    round: u32,
    winner_idx: usize,
    max_variants: usize,
    seed: u64,
    verbose: bool,
) -> Vec<Candidate> {
    let args = &winner.args;
    let rule = &winner.rule;

    let fast = match arg_u32(args, "--fast-window") {
        Some(v) => v,
        None => {
            if verbose {
                println!(
                    "  [refine] skipping winner {}: missing --fast-window",
                    winner.candidate_id
                );
            }
            return Vec::new();
        }
    };
    let slow = match arg_u32(args, "--slow-window") {
        Some(v) => v,
        None => {
            if verbose {
                println!(
                    "  [refine] skipping winner {}: missing --slow-window",
                    winner.candidate_id
                );
            }
            return Vec::new();
        }
    };
    let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
    let rsi_oversold = arg_u32(args, "--rsi-oversold").unwrap_or(30);
    let rsi_overbought = arg_u32(args, "--rsi-overbought").unwrap_or(70);
    let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
    let vol_cap = arg_f64(args, "--vol-cap").unwrap_or(0.40);
    let asset = winner.focus_asset.as_str();

    let mut candidates = Vec::new();
    let mut variant_idx = 0usize;

    match rule.as_str() {
        RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => {
            for adj_fast in adjacent_values_u32(FAST_WINDOW_SET, fast) {
                if adj_fast < slow {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        adj_fast,
                        slow,
                        rsi_window,
                        rsi_oversold,
                        rsi_overbought,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
            for adj_slow in adjacent_values_u32(SLOW_WINDOW_SET, slow) {
                if fast < adj_slow {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        fast,
                        adj_slow,
                        rsi_window,
                        rsi_oversold,
                        rsi_overbought,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
        }
        RULE_RSI_REVERSION => {
            for adj in adjacent_values_u32(RSI_WINDOW_SET, rsi_window) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    adj,
                    rsi_oversold,
                    rsi_overbought,
                    vol_window,
                    vol_cap,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
            for adj in adjacent_values_u32(RSI_OVERSOLD_SET, rsi_oversold) {
                if adj < rsi_overbought {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        fast,
                        slow,
                        rsi_window,
                        adj,
                        rsi_overbought,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
            for adj in adjacent_values_u32(RSI_OVERBOUGHT_SET, rsi_overbought) {
                if rsi_oversold < adj {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        fast,
                        slow,
                        rsi_window,
                        rsi_oversold,
                        adj,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
        }
        RULE_VOL_REGIME => {
            for adj in adjacent_values_u32(VOL_WINDOW_SET, vol_window) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    rsi_window,
                    rsi_oversold,
                    rsi_overbought,
                    adj,
                    vol_cap,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
            for adj in adjacent_values_f64(VOL_CAP_SET, vol_cap) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    rsi_window,
                    rsi_oversold,
                    rsi_overbought,
                    vol_window,
                    adj,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
        }
        _ => {
            if verbose {
                println!("  [refine] unsupported rule for refinement: {}", rule);
            }
            return Vec::new();
        }
    }

    // Asset swaps
    for &alt_asset in RESEARCH_ASSETS {
        if alt_asset != asset {
            candidates.push(make_refined_candidate(
                rule,
                alt_asset,
                fast,
                slow,
                rsi_window,
                rsi_oversold,
                rsi_overbought,
                vol_window,
                vol_cap,
                round,
                winner_idx,
                variant_idx,
            ));
            variant_idx += 1;
        }
    }

    // Local + global dedup
    let mut seen = HashSet::new();
    candidates.retain(|c| {
        let sig = param_signature(c);
        seen.insert(sig.clone()) && !loop_state.all_evaluated.contains(&sig)
    });

    // Deterministic shuffle based on seed + round + winner signature hash
    let sig_hash = param_signature_from_args(&winner.args)
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    let refinement_seed = seed.wrapping_mul(round as u64 + 1).wrapping_add(sig_hash);
    shuffle_candidates(&mut candidates, refinement_seed);

    candidates.truncate(max_variants);
    candidates
}

fn refine_around_winners(
    winners_ranked: &[CandidateReport],
    loop_state: &mut LoopState,
    refine_top: usize,
    refine_variants: usize,
    round: u32,
    seed: u64,
    verbose: bool,
) -> Vec<Candidate> {
    let mut all_variants = Vec::new();
    let mut eligible_count = 0usize;
    let mut newly_exhausted = Vec::new();

    for (idx, winner) in winners_ranked.iter().enumerate() {
        if eligible_count >= refine_top {
            break;
        }

        let sig = param_signature_from_args(&winner.args);
        if loop_state.is_center_exhausted(&sig) {
            if verbose {
                println!(
                    "  [refine] skipping exhausted center: {}",
                    winner.candidate_id
                );
            }
            continue;
        }

        let variants = refine_around_winner(
            winner,
            loop_state,
            round,
            idx,
            refine_variants,
            seed,
            verbose,
        );

        if variants.is_empty() {
            newly_exhausted.push(sig);
            if verbose {
                println!(
                    "  [refine] winner {} exhausted (0 novel variants)",
                    winner.candidate_id
                );
            }
            continue;
        }

        if verbose {
            println!(
                "  [refine] winner {} produced {} variants",
                winner.candidate_id,
                variants.len()
            );
        }

        all_variants.extend(variants);
        eligible_count += 1;
    }

    for sig in newly_exhausted {
        loop_state.mark_center_exhausted(&sig);
    }

    // Cross-winner dedup
    let mut seen = HashSet::new();
    all_variants.retain(|c| {
        let sig = param_signature(c);
        seen.insert(sig)
    });

    all_variants
}

fn print_round_summary(summary: &RoundSummary) {
    let round_best_str = summary
        .round_best
        .map_or("N/A".to_string(), |v| format!("{:.2}", v));
    let global_best_str = summary
        .global_best
        .map_or("N/A".to_string(), |v| format!("{:.2}", v));
    let imp_str = if summary.rel_improvement.abs() < 1e-10 {
        "+0.0%".to_string()
    } else {
        format!("{:+.1}%", summary.rel_improvement * 100.0)
    };
    println!(
        "Round {}: generated={} evaluated={} passed={} round_best={} global_best={} {}",
        summary.round,
        summary.generated,
        summary.evaluated,
        summary.passed,
        round_best_str,
        global_best_str,
        imp_str,
    );
}

fn main() {
    let _ = dotenv();
    let args = Args::parse();
    let doob_bin = PathBuf::from(&args.doob_bin);
    if !doob_bin.exists() {
        eprintln!("doob binary not found: {}", doob_bin.display());
        return;
    }

    let legacy_mode = args.no_loop || args.max_rounds == 1;

    println!(
        "Autoresearch run: paper-research only | strategy seed web: {} | candidates up to {}",
        args.seed_web, args.candidates
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
    if !legacy_mode {
        println!(
            "Iterative refinement: max_rounds={} patience={} min_improvement={:.2} refine_top={} refine_variants={}",
            args.max_rounds,
            args.patience,
            args.min_improvement,
            args.refine_top,
            args.refine_variants
        );
    }

    let seed_ideas = if args.seed_web {
        let ideas = fetch_exa_ideas(SEED_QUERIES, 25);
        if args.verbose {
            println!("fetched {} arXiv seeds from Exa", ideas.len());
        }
        if let Err(err) = save_seed_ideas(Path::new("reports/autoresearch-exa-ideas.json"), &ideas)
        {
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

    shuffle_candidates(&mut candidates, args.random_seed);
    if candidates.len() > args.candidates {
        candidates.truncate(args.candidates);
    }

    if args.verbose {
        println!("candidate pool: {} total", candidates.len());
        let seeded_count = candidates.iter().filter(|r| r.is_seeded).count();
        println!(
            "seeded: {} / fallback: {}",
            seeded_count,
            candidates.len() - seeded_count
        );
    }

    let train_window_sessions =
        sessions_for_window(&args.train_start, &args.train_end, args.train_sessions);
    let test_window_sessions =
        sessions_for_window(&args.test_start, &args.test_end, args.test_sessions);

    let mut loop_state = LoopState::new();

    // Load evaluation cache
    let cache_path = PathBuf::from(EVAL_CACHE_PATH);
    let eval_cache = if args.no_cache {
        if args.verbose {
            println!("evaluation cache disabled (--no-cache)");
        }
        HashMap::new()
    } else {
        let c = load_eval_cache(&cache_path);
        if args.verbose && !c.is_empty() {
            println!(
                "loaded {} cached evaluations from {}",
                c.len(),
                EVAL_CACHE_PATH
            );
        }
        c
    };

    // === Phase 1: Round 0 Exploration ===
    let novel_candidates = loop_state.retain_novel_candidates(candidates);
    let initial_count = novel_candidates.len();
    let mut round_reports = Vec::new();

    for (idx, candidate) in novel_candidates.iter().enumerate() {
        if let Some(report) = cached_evaluate_candidate(
            candidate,
            idx + 1,
            initial_count,
            &doob_bin,
            &args.train_start,
            &args.train_end,
            &args.test_start,
            &args.test_end,
            train_window_sessions,
            test_window_sessions,
            args.verbose,
            &eval_cache,
            &cache_path,
        ) {
            round_reports.push(report);
        } else if args.verbose {
            println!("  rejected: scoring gate failed or doob execution error");
        }
    }

    loop_state.record_round(0, initial_count, initial_count, round_reports, legacy_mode);
    if let Some(summary) = loop_state.round_summaries.last() {
        print_round_summary(summary);
    }

    // === Phase 2: Iterative Refinement ===
    if !legacy_mode {
        for round in 1..args.max_rounds {
            if has_converged(
                &loop_state.round_summaries,
                args.patience,
                args.min_improvement,
            ) {
                println!(
                    "Converged after {} rounds (patience={}, min_improvement={:.2})",
                    round, args.patience, args.min_improvement
                );
                break;
            }

            let all_reports: Vec<CandidateReport> = loop_state
                .recorded_results
                .iter()
                .map(|r| r.report.clone())
                .collect();
            let ranked = rank_rows(&all_reports);

            let refined = refine_around_winners(
                &ranked,
                &mut loop_state,
                args.refine_top,
                args.refine_variants,
                round,
                args.random_seed,
                args.verbose,
            );

            if refined.is_empty() {
                println!("Refinement frontier exhausted after {} rounds", round);
                break;
            }

            if args.verbose {
                println!(
                    "Round {}: {} novel refinement candidates",
                    round,
                    refined.len()
                );
            }

            let novel = loop_state.retain_novel_candidates(refined);
            let generated = novel.len();
            let mut round_reports = Vec::new();

            for (idx, candidate) in novel.iter().enumerate() {
                if let Some(report) = cached_evaluate_candidate(
                    candidate,
                    idx + 1,
                    generated,
                    &doob_bin,
                    &args.train_start,
                    &args.train_end,
                    &args.test_start,
                    &args.test_end,
                    train_window_sessions,
                    test_window_sessions,
                    args.verbose,
                    &eval_cache,
                    &cache_path,
                ) {
                    round_reports.push(report);
                } else if args.verbose {
                    println!("  rejected: scoring gate failed or doob execution error");
                }
            }

            loop_state.record_round(round, generated, generated, round_reports, legacy_mode);
            if let Some(summary) = loop_state.round_summaries.last() {
                print_round_summary(summary);
            }
        }
    }

    // === Phase 3: Final Reporting ===
    if args.verbose {
        println!(
            "completed: {} total results across {} rounds",
            loop_state.recorded_results.len(),
            loop_state.round_summaries.len()
        );
    }

    if loop_state.recorded_results.is_empty() {
        println!("No candidate passed scoring gates.");
        return;
    }

    let all_reports: Vec<CandidateReport> = loop_state
        .recorded_results
        .iter()
        .map(|r| r.report.clone())
        .collect();
    let ranked = rank_rows(&all_reports);
    let top_k = args.top.min(ranked.len());
    print_top(&ranked, top_k);
    if let Some(best) = ranked.first() {
        print_best(best);
    }

    // Round history table
    if !legacy_mode && loop_state.round_summaries.len() > 1 {
        println!("\nRound history:");
        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            "Round",
            "Generated",
            "Evaluated",
            "Passed",
            "Round Best",
            "Global Best",
            "Improvement",
        ]);
        for s in &loop_state.round_summaries {
            let rb = s
                .round_best
                .map_or("N/A".to_string(), |v| format!("{:.3}", v));
            let gb = s
                .global_best
                .map_or("N/A".to_string(), |v| format!("{:.3}", v));
            let imp = if s.rel_improvement.abs() < 1e-10 {
                "+0.0%".to_string()
            } else {
                format!("{:+.1}%", s.rel_improvement * 100.0)
            };
            table.add_row(vec![
                s.round.to_string(),
                s.generated.to_string(),
                s.evaluated.to_string(),
                s.passed.to_string(),
                rb,
                gb,
                imp,
            ]);
        }
        println!("{}", table);
    }

    if let Err(err) = save_interactive_report(
        Path::new("reports/autoresearch-top10-interactive-report.html"),
        &ranked.iter().take(10).cloned().collect::<Vec<_>>(),
    ) {
        eprintln!("Failed to write interactive report: {err}");
    }

    // Build ledger entries for top 10 with round metadata
    let top_10_sigs: HashSet<String> = ranked
        .iter()
        .take(10)
        .map(|r| param_signature_from_args(&r.args))
        .collect();
    let mut ledger_entries: Vec<RecordedResult> = Vec::new();
    let mut ledger_sigs = HashSet::new();
    for r in &loop_state.recorded_results {
        if top_10_sigs.contains(&r.signature) && ledger_sigs.insert(r.signature.clone()) {
            ledger_entries.push(r.clone());
        }
    }
    ledger_entries.sort_by(|a, b| {
        b.report
            .combined_score
            .partial_cmp(&a.report.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ledger_entries.truncate(10);

    if let Err(err) = save_ledger(
        Path::new("reports/autoresearch-ledger.jsonl"),
        &ledger_entries,
    ) {
        eprintln!("Failed to save ledger: {err}");
    }

    let _ = Command::new("open")
        .arg("reports/autoresearch-top10-interactive-report.html")
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary(round: u32, global_best: Option<f64>, rel_improvement: f64) -> RoundSummary {
        RoundSummary {
            round,
            generated: 10,
            evaluated: 10,
            passed: 5,
            round_best: global_best,
            global_best,
            rel_improvement,
        }
    }

    // === Convergence tests ===

    #[test]
    fn test_has_converged_false_before_patience() {
        let summaries = vec![
            make_summary(0, Some(1.0), 0.0),
            make_summary(1, Some(1.005), 0.005),
            make_summary(2, Some(1.008), 0.003),
        ];
        // patience=3, only 2 stale rounds (rounds 1 and 2 have < 0.02 improvement)
        assert!(!has_converged(&summaries, 3, 0.02));
    }

    #[test]
    fn test_has_converged_true_after_patience_small_gains() {
        let summaries = vec![
            make_summary(0, Some(1.0), 0.0),
            make_summary(1, Some(1.005), 0.005),
            make_summary(2, Some(1.008), 0.003),
            make_summary(3, Some(1.010), 0.002),
        ];
        // patience=3, all 3 rounds after round 0 have < 0.02 improvement
        assert!(has_converged(&summaries, 3, 0.02));
    }

    #[test]
    fn test_has_converged_false_on_meaningful_gain() {
        let summaries = vec![
            make_summary(0, Some(1.0), 0.0),
            make_summary(1, Some(1.005), 0.005),
            make_summary(2, Some(1.10), 0.095), // meaningful improvement resets counter
            make_summary(3, Some(1.105), 0.005),
        ];
        assert!(!has_converged(&summaries, 3, 0.02));
    }

    #[test]
    fn test_has_converged_handles_zero_or_negative_baseline() {
        // Global best is 0 → denominator becomes max(0, 1.0) = 1.0
        let summaries = vec![
            make_summary(0, Some(0.0), 0.0),
            make_summary(1, Some(0.005), 0.005),
            make_summary(2, Some(0.008), 0.003),
            make_summary(3, Some(0.010), 0.002),
        ];
        assert!(has_converged(&summaries, 3, 0.02));

        // Negative baseline
        let summaries_neg = vec![
            make_summary(0, Some(-2.0), 0.0),
            make_summary(1, Some(-1.99), 0.005),
            make_summary(2, Some(-1.98), 0.005),
            make_summary(3, Some(-1.97), 0.005),
        ];
        assert!(has_converged(&summaries_neg, 3, 0.02));
    }

    #[test]
    fn test_has_converged_round_0_alone_never_converges() {
        let summaries = vec![make_summary(0, Some(1.0), 0.0)];
        assert!(!has_converged(&summaries, 1, 0.02));
    }

    // === Adjacency tests ===

    #[test]
    fn test_adjacent_first_element() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 6);
        assert_eq!(result, vec![8]);
    }

    #[test]
    fn test_adjacent_last_element() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 12);
        assert_eq!(result, vec![10]);
    }

    #[test]
    fn test_adjacent_middle_element() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 8);
        assert_eq!(result, vec![6, 10]);
    }

    #[test]
    fn test_adjacent_missing_value() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 9);
        assert_eq!(result, vec![8, 10]);
    }

    #[test]
    fn test_adjacent_no_duplicates() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 10);
        assert_eq!(result.len(), 2);
        assert_ne!(result[0], result[1]);
    }

    #[test]
    fn test_adjacent_f64_middle() {
        let set = &[0.20, 0.25, 0.30, 0.35];
        let result = adjacent_values_f64(set, 0.25);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.20).abs() < 1e-6);
        assert!((result[1] - 0.30).abs() < 1e-6);
    }

    #[test]
    fn test_adjacent_f64_missing() {
        let set = &[0.20, 0.30, 0.40];
        let result = adjacent_values_f64(set, 0.25);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.20).abs() < 1e-6);
        assert!((result[1] - 0.30).abs() < 1e-6);
    }

    // === Signature tests ===

    #[test]
    fn test_signature_excludes_hypothesis_id() {
        let args_with_hyp = vec![
            "--asset".to_string(),
            "SPY".to_string(),
            "--rule".to_string(),
            "trend_momentum".to_string(),
            "--fast-window".to_string(),
            "12".to_string(),
            "--slow-window".to_string(),
            "50".to_string(),
            "--hypothesis-id".to_string(),
            "seed-1-0".to_string(),
        ];
        let args_without_hyp = vec![
            "--asset".to_string(),
            "SPY".to_string(),
            "--rule".to_string(),
            "trend_momentum".to_string(),
            "--fast-window".to_string(),
            "12".to_string(),
            "--slow-window".to_string(),
            "50".to_string(),
        ];
        assert_eq!(
            param_signature_from_args(&args_with_hyp),
            param_signature_from_args(&args_without_hyp)
        );
    }

    #[test]
    fn test_signature_stable_float_formatting() {
        let args1 = vec![
            "--vol-cap".to_string(),
            "0.40".to_string(),
            "--asset".to_string(),
            "SPY".to_string(),
        ];
        let args2 = vec![
            "--vol-cap".to_string(),
            "0.4".to_string(),
            "--asset".to_string(),
            "SPY".to_string(),
        ];
        assert_eq!(
            param_signature_from_args(&args1),
            param_signature_from_args(&args2)
        );
    }

    #[test]
    fn test_signature_sorted_keys() {
        let args1 = vec![
            "--asset".to_string(),
            "SPY".to_string(),
            "--rule".to_string(),
            "trend_momentum".to_string(),
        ];
        let args2 = vec![
            "--rule".to_string(),
            "trend_momentum".to_string(),
            "--asset".to_string(),
            "SPY".to_string(),
        ];
        assert_eq!(
            param_signature_from_args(&args1),
            param_signature_from_args(&args2)
        );
    }

    // === Candidate construction tests ===

    #[test]
    fn test_refined_id_format() {
        let c = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "SPY",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            2,
            3,
            7,
        );
        assert_eq!(c.candidate_id, "refine-R2-W03-V007");
    }

    #[test]
    fn test_stable_arg_ordering() {
        let c = make_refined_candidate(
            RULE_RSI_REVERSION,
            "QQQ",
            10,
            35,
            14,
            28,
            72,
            20,
            0.40,
            1,
            0,
            0,
        );
        // Verify args contain expected key-value pairs
        assert_eq!(arg_value(&c.args, "--asset"), Some("QQQ"));
        assert_eq!(arg_value(&c.args, "--rule"), Some(RULE_RSI_REVERSION));
        assert_eq!(arg_u32(&c.args, "--fast-window"), Some(10));
        assert_eq!(arg_u32(&c.args, "--slow-window"), Some(35));
        assert_eq!(arg_u32(&c.args, "--rsi-window"), Some(14));
        assert_eq!(arg_u32(&c.args, "--rsi-oversold"), Some(28));
        assert_eq!(arg_u32(&c.args, "--rsi-overbought"), Some(72));
        assert_eq!(arg_u32(&c.args, "--vol-window"), Some(20));
        assert!((arg_f64(&c.args, "--vol-cap").unwrap() - 0.40).abs() < 1e-6);
    }

    // === Refinement logic tests ===

    #[test]
    fn test_fast_less_than_slow_invariant() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants = refine_around_winner(&winner, &loop_state, 1, 0, 100, 42, false);
        for v in &variants {
            let fast = arg_u32(&v.args, "--fast-window").unwrap();
            let slow = arg_u32(&v.args, "--slow-window").unwrap();
            assert!(
                fast < slow,
                "fast ({fast}) must be < slow ({slow}) for {}",
                v.candidate_id
            );
        }
    }

    #[test]
    fn test_oversold_less_than_overbought_invariant() {
        let winner = CandidateReport {
            candidate_id: "test-rsi".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "QQQ".to_string(),
                "--rule".to_string(),
                RULE_RSI_REVERSION.to_string(),
                "--fast-window".to_string(),
                "10".to_string(),
                "--slow-window".to_string(),
                "35".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "28".to_string(),
                "--rsi-overbought".to_string(),
                "72".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_RSI_REVERSION.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "QQQ".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants = refine_around_winner(&winner, &loop_state, 1, 0, 100, 42, false);
        for v in &variants {
            let os = arg_u32(&v.args, "--rsi-oversold").unwrap();
            let ob = arg_u32(&v.args, "--rsi-overbought").unwrap();
            assert!(
                os < ob,
                "oversold ({os}) must be < overbought ({ob}) for {}",
                v.candidate_id
            );
        }
    }

    #[test]
    fn test_dedup_against_global_seen() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };

        // Get initial variants
        let loop_state = LoopState::new();
        let variants_1 = refine_around_winner(&winner, &loop_state, 1, 0, 100, 42, false);
        assert!(!variants_1.is_empty());

        // Mark all those signatures as seen
        let mut loop_state_2 = LoopState::new();
        for v in &variants_1 {
            loop_state_2.all_evaluated.insert(param_signature(v));
        }

        // Second refinement should produce no novel variants
        let variants_2 = refine_around_winner(&winner, &loop_state_2, 2, 0, 100, 42, false);
        assert!(
            variants_2.is_empty(),
            "expected 0 novel variants, got {}",
            variants_2.len()
        );
    }

    #[test]
    fn test_exhausted_winners_skipped() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };

        let mut loop_state = LoopState::new();
        let sig = param_signature_from_args(&winner.args);
        loop_state.mark_center_exhausted(&sig);

        let result = refine_around_winners(&[winner], &mut loop_state, 5, 20, 1, 42, false);
        assert!(result.is_empty());
    }

    #[test]
    fn test_deterministic_ordering() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let v1 = refine_around_winner(&winner, &loop_state, 1, 0, 100, 42, false);
        let v2 = refine_around_winner(&winner, &loop_state, 1, 0, 100, 42, false);
        let ids1: Vec<&str> = v1.iter().map(|c| c.candidate_id.as_str()).collect();
        let ids2: Vec<&str> = v2.iter().map(|c| c.candidate_id.as_str()).collect();
        assert_eq!(ids1, ids2, "same seed should produce same ordering");
    }

    // === Ledger tests ===

    #[test]
    fn test_ledger_round_metadata() {
        let report = CandidateReport {
            candidate_id: "test-1".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };
        let recorded = RecordedResult {
            round: Some(2),
            signature: "test-sig".to_string(),
            report,
        };

        let dir = std::env::temp_dir().join("doob_test_ledger");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_ledger.jsonl");
        let _ = std::fs::remove_file(&path);

        save_ledger(&path, &[recorded]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["round"], json!(2));
        assert_eq!(parsed["candidate_id"], json!("test-1"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ledger_legacy_backward_compatible() {
        let report = CandidateReport {
            candidate_id: "test-legacy".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };
        let recorded = RecordedResult {
            round: None, // legacy mode
            signature: "test-sig".to_string(),
            report,
        };

        let dir = std::env::temp_dir().join("doob_test_ledger_legacy");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_ledger_legacy.jsonl");
        let _ = std::fs::remove_file(&path);

        save_ledger(&path, &[recorded]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        // Legacy mode: no "round" field
        assert!(
            parsed.get("round").is_none(),
            "legacy mode should not include round field"
        );
        assert_eq!(parsed["candidate_id"], json!("test-legacy"));

        let _ = std::fs::remove_file(&path);
    }

    // === LoopState tests ===

    #[test]
    fn test_loop_state_retain_novel_candidates() {
        let mut state = LoopState::new();
        let c1 = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "SPY",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            0,
            0,
            0,
        );
        let c2 = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "QQQ",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            0,
            0,
            1,
        );
        // c3 is a duplicate of c1 (same params, different hypothesis-id due to different variant_idx but same param signature)
        let c3 = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "SPY",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            0,
            0,
            2,
        );

        let novel = state.retain_novel_candidates(vec![c1, c2, c3]);
        // c1 and c2 should pass, c3 should be deduped
        assert_eq!(novel.len(), 2);
    }

    #[test]
    fn test_loop_state_current_best_score() {
        let mut state = LoopState::new();
        assert!(state.current_best_score().is_none());

        let report = CandidateReport {
            candidate_id: "test".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            is_seeded: false,
        };
        state.recorded_results.push(RecordedResult {
            round: Some(0),
            signature: "test".to_string(),
            report,
        });
        assert!((state.current_best_score().unwrap() - 0.93).abs() < 1e-6);
    }
}
