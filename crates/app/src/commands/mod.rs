use anyhow::Result;
use dex_simulator_core::config::AppConfig;

use crate::cli::Commands;

pub mod listen;
pub mod serve;
pub mod simulate;
pub mod state;
pub mod utils;

/// 根据子命令执行对应逻辑。
pub async fn run(config: &AppConfig, command: Commands) -> Result<()> {
    match command {
        Commands::Serve { dry_run } => serve::serve(config, dry_run).await,
        Commands::DumpPoolState { pool } => state::dump_pool_state(config, pool).await,
        Commands::SimulatePancakeV2Swap { pool, amount } => {
            simulate::simulate_swap(config, pool, amount).await
        }
        Commands::Listen { sample_events } => listen::listen(config, sample_events).await,
        Commands::BenchSwap {
            pool,
            amount,
            iterations,
        } => simulate::bench_swap(config, pool, amount, iterations).await,
        Commands::SimulatePancakeV3Swap { amount, reverse } => {
            simulate::simulate_swap_v3(amount, reverse).await
        }
    }
}
