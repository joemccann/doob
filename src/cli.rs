/// Unified CLI entrypoint using clap derive.
///
/// Usage:
///     doob run overnight-drift [OPTIONS]
///     doob run intraday-drift [OPTIONS]
///     doob run breadth-washout [OPTIONS]
///     doob run ndx100-sma-breadth [OPTIONS]
///     doob list-strategies
///     doob list-presets

use clap::{Parser, Subcommand, ValueEnum};

use crate::strategies::breadth_dual_ma::BreadthDualMaArgs;
use crate::strategies::breadth_ma::BreadthMaArgs;
use crate::strategies::breadth_washout::BreadthWashoutArgs;
use crate::strategies::intraday_drift::IntradayDriftArgs;
use crate::strategies::ndx100_breadth_washout::Ndx100BreadthWashoutArgs;
use crate::strategies::ndx100_sma_breadth::Ndx100SmaBreadthArgs;
use crate::strategies::overnight_drift::OvernightDriftArgs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Md,
}

#[derive(Parser)]
#[command(name = "doob", about = "Quantitative strategy research and backtesting")]
pub struct Cli {
    /// Output format: text (default), json (structured), or md (markdown)
    #[arg(long, default_value = "text", global = true)]
    pub output: OutputFormat,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a backtesting strategy
    Run {
        #[command(subcommand)]
        strategy: StrategyCommand,
    },
    /// List available strategies
    ListStrategies,
    /// List available presets
    ListPresets,
}

#[derive(Subcommand)]
pub enum StrategyCommand {
    /// Buy close, sell next open; optional VIX filter
    OvernightDrift(OvernightDriftArgs),
    /// Buy open, sell close same day; optional short mode
    IntradayDrift(IntradayDriftArgs),
    /// Generic breadth signal across any universe
    BreadthWashout(BreadthWashoutArgs),
    /// Single MA breadth: % below/above N-day MA (default 50-day)
    BreadthMa(BreadthMaArgs),
    /// Dual MA breadth: close < short MA AND close > long MA
    BreadthDualMa(BreadthDualMaArgs),
    /// NDX-100 SMA breadth analysis + forward returns
    Ndx100SmaBreadth(Ndx100SmaBreadthArgs),
    /// NDX-100 breadth washout wrapper
    Ndx100BreadthWashout(Ndx100BreadthWashoutArgs),
}

pub fn list_strategies() {
    println!("Available strategies:");
    println!("  breadth-dual-ma");
    println!("  breadth-ma");
    println!("  breadth-washout");
    println!("  intraday-drift");
    println!("  ndx100-breadth-washout");
    println!("  ndx100-sma-breadth");
    println!("  overnight-drift");
}

pub fn list_presets() {
    let presets = crate::data::presets::list_presets();
    if presets.is_empty() {
        println!("No presets found.");
        return;
    }
    println!("Available presets:");
    for name in presets {
        println!("  {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_list_strategies_prints_all() {
        // Just ensure list_strategies doesn't panic
        list_strategies();
    }

    #[test]
    fn test_parse_run_overnight_drift() {
        let cli = Cli::try_parse_from(&["doob", "run", "overnight-drift", "--no-plots"]).unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::OvernightDrift(args) => {
                    assert!(args.no_plots);
                    assert!(!args.no_vix_filter);
                    assert_eq!(args.capital, 1_000_000.0);
                }
                _ => panic!("Expected OvernightDrift"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_run_intraday_drift() {
        let cli = Cli::try_parse_from(&[
            "doob",
            "run",
            "intraday-drift",
            "--ticker",
            "QQQ",
            "--short",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::IntradayDrift(args) => {
                    assert_eq!(args.ticker, "QQQ");
                    assert!(args.short);
                }
                _ => panic!("Expected IntradayDrift"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_list_strategies() {
        let cli = Cli::try_parse_from(&["doob", "list-strategies"]).unwrap();
        assert!(matches!(cli.command, Command::ListStrategies));
    }

    #[test]
    fn test_parse_list_presets() {
        let cli = Cli::try_parse_from(&["doob", "list-presets"]).unwrap();
        assert!(matches!(cli.command, Command::ListPresets));
    }

    #[test]
    fn test_no_args_fails() {
        let result = Cli::try_parse_from(&["doob"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_command_fails() {
        let result = Cli::try_parse_from(&["doob", "bogus"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_no_strategy_fails() {
        let result = Cli::try_parse_from(&["doob", "run"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_breadth_washout() {
        let cli = Cli::try_parse_from(&[
            "doob",
            "run",
            "breadth-washout",
            "--signal-mode",
            "overbought",
            "--threshold",
            "70",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::BreadthWashout(args) => {
                    assert_eq!(args.signal_mode, "overbought");
                    assert_eq!(args.threshold, Some(70.0));
                }
                _ => panic!("Expected BreadthWashout"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_ndx100_sma_breadth() {
        let cli = Cli::try_parse_from(&[
            "doob",
            "run",
            "ndx100-sma-breadth",
            "--end-date",
            "2026-01-15",
            "--sessions",
            "100",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::Ndx100SmaBreadth(args) => {
                    assert_eq!(args.end_date, "2026-01-15");
                    assert_eq!(args.sessions, 100);
                }
                _ => panic!("Expected Ndx100SmaBreadth"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_overnight_drift_with_dates() {
        let cli = Cli::try_parse_from(&[
            "doob",
            "run",
            "overnight-drift",
            "--start-date",
            "2020-01-01",
            "--end-date",
            "2025-12-31",
            "--capital",
            "500000",
            "--no-vix-filter",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::OvernightDrift(args) => {
                    assert_eq!(args.start_date, Some("2020-01-01".to_string()));
                    assert_eq!(args.end_date, Some("2025-12-31".to_string()));
                    assert_eq!(args.capital, 500000.0);
                    assert!(args.no_vix_filter);
                }
                _ => panic!("Expected OvernightDrift"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_ndx100_breadth_washout() {
        let cli = Cli::try_parse_from(&["doob", "run", "ndx100-breadth-washout"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Run {
                strategy: StrategyCommand::Ndx100BreadthWashout(_)
            }
        ));
    }

    #[test]
    fn test_output_default_is_text() {
        let cli = Cli::try_parse_from(&["doob", "list-strategies"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Text);
    }

    #[test]
    fn test_output_json_flag() {
        let cli =
            Cli::try_parse_from(&["doob", "--output", "json", "run", "overnight-drift"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn test_output_json_flag_after_subcommand() {
        let cli =
            Cli::try_parse_from(&["doob", "run", "overnight-drift", "--output", "json"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn test_output_md_flag() {
        let cli =
            Cli::try_parse_from(&["doob", "--output", "md", "run", "overnight-drift"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Md);
    }

    #[test]
    fn test_parse_breadth_ma() {
        let cli = Cli::try_parse_from(&[
            "doob",
            "run",
            "breadth-ma",
            "--short-period",
            "50",
            "--threshold",
            "80",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::BreadthMa(args) => {
                    assert_eq!(args.short_period, 50);
                    assert_eq!(args.threshold, 80.0);
                }
                _ => panic!("Expected BreadthMa"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_breadth_dual_ma() {
        let cli = Cli::try_parse_from(&[
            "doob",
            "run",
            "breadth-dual-ma",
            "--short-period",
            "50",
            "--long-period",
            "200",
            "--threshold",
            "40",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::BreadthDualMa(args) => {
                    assert_eq!(args.short_period, 50);
                    assert_eq!(args.long_period, 200);
                    assert_eq!(args.threshold, 40.0);
                }
                _ => panic!("Expected BreadthDualMa"),
            },
            _ => panic!("Expected Run command"),
        }
    }
}
