use anyhow::{anyhow, Result};
use ethers::utils::__serde_json::json;
use std::sync::Arc;
use std::time::Instant;

use super::utils::{
    format_address, format_identifier, parse_pool_identifier, sample_pool, sample_pool_identifier,
    snapshot_to_json, stats_summary, stats_summary_u128,
};
use dex_simulator_core::config::AppConfig;
use dex_simulator_core::dex::pancake_v2::{PancakeV2PoolState, PancakeV2SwapCalculator};
use dex_simulator_core::dex::pancake_v3::calculator::PancakeV3SwapCalculator;
use dex_simulator_core::path::{GraphPathFinder, PathConstraints, PathFinder};
use dex_simulator_core::simulation::{
    SimulationError, SwapCalculator, SwapSimulationRequest, SwapSimulationResult,
};
use dex_simulator_core::state::repository::{InMemoryPoolRepository, PoolRepository};
use dex_simulator_core::state::snapshot::{FileSnapshotStore, SnapshotStore};
use dex_simulator_core::state::{PoolState, StateError};
use dex_simulator_core::token_graph::TokenGraph;
use dex_simulator_core::types::{Amount, Asset, PathLeg, PoolIdentifier, PoolType};
use ethers::types::{Address, U256};
use ethers::utils::__serde_json;
use std::str::FromStr;

/// 执行 PancakeSwap V2 swap 模拟。
pub async fn simulate_swap(config: &AppConfig, pool_id: String, amount: u128) -> Result<()> {
    let (identifier, repo, path) = prepare_simulation(config, &pool_id).await?;
    let repo_trait: Arc<dyn PoolRepository> = repo.clone();
    let calculator = PancakeV2SwapCalculator::new(repo_trait, 25);
    let request = SwapSimulationRequest {
        path,
        amount_in: Amount(amount),
        min_amount_out: None,
        sandbox: true,
    };

    let start = Instant::now();
    match calculator.simulate(request).await {
        Ok(result) => {
            let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
            let path_json: Vec<_> = result
                .executed_path
                .iter()
                .map(|leg| {
                    json!({
                        "pool": format_address(leg.pool.address),
                        "input": format_address(leg.input.address),
                        "output": format_address(leg.output.address),
                        "symbol_in": leg.input.symbol,
                        "symbol_out": leg.output.symbol,
                        "fee_bps": leg.fee,
                    })
                })
                .collect();
            let output = json!({
                "pool_id": format_identifier(&identifier),
                "amount_in": amount,
                "amount_out": result.amount_out.0,
                "fees_paid": result.fees_paid.0,
                "slippage_bps": result.slippage_bps,
                "duration_ms": duration_ms,
                "calc_duration_ms": result.metrics.calc_duration_ms,
                "steps": result.metrics.steps.iter().map(|step| json!({
                    "name": step.name,
                    "duration_ms": step.duration_ms,
                })).collect::<Vec<_>>(),
                "path": path_json,
                "final_snapshot": result
                    .snapshots
                    .last()
                    .map(snapshot_to_json)
                    .unwrap_or(json!(null)),
            });
            println!("{}", __serde_json::to_string_pretty(&output)?);
        }
        Err(err) => {
            log::error!("模拟失败: {}", err);
        }
    }
    Ok(())
}

/// 基准测试。
pub async fn bench_swap(
    config: &AppConfig,
    pool_id: String,
    amount: u128,
    iterations: u32,
) -> Result<()> {
    if iterations == 0 {
        return Err(anyhow!("iterations 必须大于 0"));
    }
    let (identifier, repo, path) = prepare_simulation(config, &pool_id).await?;
    let repo_trait: Arc<dyn PoolRepository> = repo.clone();
    let calculator = PancakeV2SwapCalculator::new(repo_trait, 25);

    let mut durations = Vec::with_capacity(iterations as usize);
    let mut calc_durations = Vec::with_capacity(iterations as usize);
    let mut outputs = Vec::with_capacity(iterations as usize);
    let mut success = 0u32;
    let mut last_result: Option<SwapSimulationResult> = None;

    for _ in 0..iterations {
        let request = SwapSimulationRequest {
            path: path.clone(),
            amount_in: Amount(amount),
            min_amount_out: None,
            sandbox: true,
        };
        let iter_start = Instant::now();
        match calculator.simulate(request).await {
            Ok(result) => {
                let elapsed = iter_start.elapsed().as_secs_f64() * 1000.0;
                durations.push(elapsed);
                calc_durations.push(result.metrics.calc_duration_ms);
                outputs.push(result.amount_out.0);
                success += 1;
                last_result = Some(result);
            }
            Err(err) => {
                log::warn!("基准测试迭代失败: {}", err);
            }
        }
    }

    let failure = iterations - success;
    let summary = json!({
        "pool_id": format_identifier(&identifier),
        "iterations": iterations,
        "success": success,
        "failure": failure,
        "duration_ms": stats_summary(&durations),
        "calc_duration_ms": stats_summary(&calc_durations),
        "amount_out": stats_summary_u128(&outputs),
        "last_result": last_result
            .as_ref()
            .map(|res| json!({
                "amount_out": res.amount_out.0,
                "fees_paid": res.fees_paid.0,
                "slippage_bps": res.slippage_bps,
                "path": res.executed_path.iter().map(|leg| json!({
                    "pool": format_address(leg.pool.address),
                    "input": format_address(leg.input.address),
                    "output": format_address(leg.output.address),
                    "symbol_in": leg.input.symbol,
                    "symbol_out": leg.output.symbol,
                    "fee_bps": leg.fee,
                })).collect::<Vec<_>>(),
                "final_snapshot": res.snapshots.last().map(snapshot_to_json).unwrap_or(json!(null)),
            }))
            .unwrap_or(json!(null)),
    });

    println!("{}", __serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// 演示 PancakeSwap V3 模拟。
pub async fn simulate_swap_v3(amount: u128, reverse: bool) -> Result<()> {
    let (identifier, _repo, path, calculator) = build_v3_components(reverse).await?;
    let request = SwapSimulationRequest {
        path,
        amount_in: Amount(amount),
        min_amount_out: None,
        sandbox: true,
    };

    let start = Instant::now();
    match calculator.simulate(request).await {
        Ok(result) => {
            let output = json!({
                "pool_id": format_identifier(&identifier),
                "amount_in": amount,
                "amount_out": result.amount_out.0,
                "fees_paid": result.fees_paid.0,
                "duration_ms": start.elapsed().as_secs_f64() * 1000.0,
                "path": result.executed_path.iter().map(|leg| json!({
                    "pool": format_address(leg.pool.address),
                    "input": format_address(leg.input.address),
                    "output": format_address(leg.output.address),
                    "symbol_in": leg.input.symbol,
                    "symbol_out": leg.output.symbol,
                    "fee_bps": leg.fee,
                })).collect::<Vec<_>>(),
                "metrics": {
                    "calc_duration_ms": result.metrics.calc_duration_ms,
                    "steps": result.metrics.steps.iter().map(|step| json!({
                        "name": step.name,
                        "duration_ms": step.duration_ms,
                    })).collect::<Vec<_>>()
                }
            });
            println!("{}", __serde_json::to_string_pretty(&output)?);
        }
        Err(err) => {
            log::error!("V3 模拟失败: {}", err);
        }
    }
    Ok(())
}

async fn prepare_simulation(
    config: &AppConfig,
    pool_id: &str,
) -> Result<(PoolIdentifier, Arc<InMemoryPoolRepository>, Vec<PathLeg>)> {
    let identifier = if pool_id == "sample" {
        sample_pool_identifier(config.network.chain_id)
    } else {
        parse_pool_identifier(pool_id).ok_or_else(|| anyhow!("无法解析池子标识"))?
    };

    let repo = Arc::new(InMemoryPoolRepository::new());
    let store = FileSnapshotStore::new(&config.runtime.snapshot_path);
    if let Some(snapshot) = store.load(&identifier).await? {
        repo.upsert(snapshot).await?;
    } else {
        let (pool_id, token0, token1) = sample_pool(config.network.chain_id);
        let mut state = PancakeV2PoolState::new(pool_id.clone(), token0.clone(), token1.clone());
        state.set_reserves(1_000_000_000_000_000_000u128, 500_000_000_000_000_000u128);
        repo.upsert(state.to_snapshot()?).await?;
        log::info!("使用示例池子进行模拟: {:?}", pool_id);
    }

    let path = build_graph_path(&repo, &identifier, 4).await?;
    Ok((identifier, repo, path))
}

async fn build_graph_path(
    repo: &Arc<InMemoryPoolRepository>,
    identifier: &PoolIdentifier,
    max_hops: usize,
) -> Result<Vec<PathLeg>, SimulationError> {
    let snapshots = repo.list().await.map_err(SimulationError::State)?;
    let mut graph = TokenGraph::new();
    let mut source_asset: Option<Asset> = None;
    let mut target_asset: Option<Asset> = None;

    for snapshot in snapshots {
        let state = PancakeV2PoolState::from_snapshot(&snapshot).map_err(SimulationError::State)?;
        graph.add_pool(
            snapshot.id.clone(),
            vec![state.token0().clone(), state.token1().clone()],
        );
        if snapshot.id == *identifier {
            source_asset = Some(state.token0().clone());
            target_asset = Some(state.token1().clone());
        }
    }

    let start = source_asset.ok_or_else(|| SimulationError::State(StateError::NotFound))?;
    let end = target_asset.ok_or_else(|| SimulationError::State(StateError::NotFound))?;
    let finder = GraphPathFinder::new(graph);
    let paths = finder
        .best_paths(
            start,
            end,
            Some(PathConstraints {
                max_hops,
                whitelist: Vec::new(),
            }),
        )
        .await
        .map_err(|err| SimulationError::Calculation(err.to_string()))?;
    paths
        .into_iter()
        .next()
        .ok_or_else(|| SimulationError::Calculation("未找到可用路径".into()))
}

async fn build_v3_components(
    reverse: bool,
) -> Result<(
    PoolIdentifier,
    Arc<InMemoryPoolRepository>,
    Vec<PathLeg>,
    PancakeV3SwapCalculator,
)> {
    use dex_simulator_core::dex::pancake_v3::event::PancakeV3Event;
    use dex_simulator_core::dex::pancake_v3::state::PancakeV3PoolState;

    let pool_id = PoolIdentifier {
        chain_id: 56,
        dex: "pancakeswap".into(),
        address: Address::from_str("0x0000000000000000000000000000000000000333")
            .unwrap_or_else(|_| Address::repeat_byte(0x44)),
        pool_type: PoolType::PancakeV3,
    };
    let token0 = Asset {
        address: Address::from_str("0x0000000000000000000000000000000000000011")
            .unwrap_or_else(|_| Address::repeat_byte(0x11)),
        symbol: "WBNB".into(),
        decimals: 18,
    };
    let token1 = Asset {
        address: Address::from_str("0x0000000000000000000000000000000000000022")
            .unwrap_or_else(|_| Address::repeat_byte(0x22)),
        symbol: "USDT".into(),
        decimals: 18,
    };

    let mut state = PancakeV3PoolState::new(pool_id.clone(), token0.clone(), token1.clone());
    state
        .update_from_event(&PancakeV3Event::Initialize {
            sqrt_price_x96: U256::from_dec_str("79228162514264337593543950336")
                .unwrap_or_else(|_| U256::from(0)),
            tick: 0,
        })
        .map_err(|err| anyhow!(err.to_string()))?;
    state
        .update_from_event(&PancakeV3Event::Mint {
            sender: Address::zero(),
            owner: Address::zero(),
            tick_lower: -600,
            tick_upper: 600,
            liquidity: U256::from(1_000_000u64),
            amount0: U256::from(500_000u64),
            amount1: U256::from(500_000u64),
        })
        .map_err(|err| anyhow!(err.to_string()))?;

    let repo = Arc::new(InMemoryPoolRepository::new());
    repo.upsert(state.snapshot()).await?;

    let path_leg = if !reverse {
        PathLeg {
            pool: pool_id.clone(),
            input: token0.clone(),
            output: token1.clone(),
            fee: Some(100),
        }
    } else {
        PathLeg {
            pool: pool_id.clone(),
            input: token1.clone(),
            output: token0.clone(),
            fee: Some(100),
        }
    };

    let repo_trait: Arc<dyn PoolRepository> = repo.clone();
    let calculator = PancakeV3SwapCalculator::new(repo_trait);
    Ok((pool_id, repo, vec![path_leg], calculator))
}
