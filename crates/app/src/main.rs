mod cli;
mod commands;
mod shared;

use anyhow::Result;
use clap::Parser;
use tokio::runtime::Runtime;

use cli::Cli;
use commands::run;
use dex_simulator_core::config::{init_global_config, load_config};
use dex_simulator_core::logging::init_logging;

fn main() -> Result<()> {
    let Cli {
        config: config_path,
        command,
    } = Cli::parse();
    let config = load_config(&config_path)?;
    init_global_config(config.clone())?;
    init_logging(&config.logging)?;

    let rt = Runtime::new()?;
    rt.block_on(async move { run(&config, command).await })
}
