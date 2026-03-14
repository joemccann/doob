use anyhow::Result;
use clap::Parser;

use doob::cli::{Cli, Command, StrategyCommand};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run { strategy } => match strategy {
            StrategyCommand::OvernightDrift(args) => {
                doob::strategies::overnight_drift::run(&args)?;
            }
            StrategyCommand::IntradayDrift(args) => {
                doob::strategies::intraday_drift::run(&args)?;
            }
            StrategyCommand::BreadthWashout(args) => {
                doob::strategies::breadth_washout::run(&args)?;
            }
            StrategyCommand::Ndx100SmaBreadth(args) => {
                doob::strategies::ndx100_sma_breadth::run(&args)?;
            }
            StrategyCommand::Ndx100BreadthWashout(args) => {
                doob::strategies::ndx100_breadth_washout::run(&args)?;
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
