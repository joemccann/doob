use anyhow::Result;
use clap::Parser;

use doob::cli::{Cli, Command, OutputFormat, StrategyCommand};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let fmt = cli.output;

    // Only init tracing for text output (avoid polluting JSON stdout)
    if fmt == OutputFormat::Text {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::INFO.into()),
            )
            .init();
    }

    match cli.command {
        Command::Run { strategy } => match strategy {
            StrategyCommand::OvernightDrift(args) => {
                doob::strategies::overnight_drift::run(&args, fmt)?;
            }
            StrategyCommand::IntradayDrift(args) => {
                doob::strategies::intraday_drift::run(&args, fmt)?;
            }
            StrategyCommand::BreadthWashout(args) => {
                doob::strategies::breadth_washout::run(&args, fmt)?;
            }
            StrategyCommand::Ndx100SmaBreadth(args) => {
                doob::strategies::ndx100_sma_breadth::run(&args, fmt)?;
            }
            StrategyCommand::Ndx100BreadthWashout(args) => {
                doob::strategies::ndx100_breadth_washout::run(&args, fmt)?;
            }
        },
        Command::ListStrategies => {
            doob::cli::list_strategies();
        }
        Command::ListPresets => {
            doob::cli::list_presets();
        }
    }

    Ok(())
}
