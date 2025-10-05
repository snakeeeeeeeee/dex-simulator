use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// CLI 入口参数定义。
#[derive(Parser, Debug)]
#[command(author, version, about = "本地 DEX 模拟器 CLI", long_about = None)]
pub struct Cli {
    /// 配置文件路径
    #[arg(
        short,
        long,
        value_name = "FILE",
        default_value = "config/default.yaml"
    )]
    pub config: PathBuf,

    /// 子命令
    #[command(subcommand)]
    pub command: Commands,
}

/// 支持的子命令。
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 启动占位 REST 服务
    Serve {
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// 打印指定池子的本地状态
    DumpPoolState {
        #[arg(long, value_name = "POOL_ID")]
        pool: String,
    },
    /// 执行 PancakeSwap V2 模拟
    SimulatePancakeV2Swap {
        #[arg(long, value_name = "POOL_ID", default_value = "sample")]
        pool: String,
        #[arg(
            long,
            value_name = "AMOUNT",
            default_value_t = 1_000_000_000_000_000_000u128
        )]
        amount: u128,
    },
    /// 启动监听器并消费若干模拟事件
    Listen {
        #[arg(long, default_value_t = 3u64)]
        sample_events: u64,
    },
    /// 运行多次模拟以收集性能指标
    BenchSwap {
        #[arg(long, value_name = "POOL_ID", default_value = "sample")]
        pool: String,
        #[arg(
            long,
            value_name = "AMOUNT",
            default_value_t = 1_000_000_000_000_000_000u128
        )]
        amount: u128,
        #[arg(long, default_value_t = 50u32)]
        iterations: u32,
    },
    /// 演示 PancakeSwap V3 模拟
    SimulatePancakeV3Swap {
        #[arg(
            long,
            value_name = "AMOUNT",
            default_value_t = 1_000_000_000_000_000u128
        )]
        amount: u128,
        #[arg(long, default_value_t = false)]
        reverse: bool,
    },
}
