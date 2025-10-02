use async_trait::async_trait;
use ethers::types::U256;
use std::sync::Arc;

use crate::dex::pancake_v3::state::PancakeV3PoolState;
use crate::dex::traits::ConcentratedLiquidityPoolState;
use crate::simulation::{
    SimulationError, SwapCalculator, SwapSimulationRequest, SwapSimulationResult,
};
use crate::state::repository::PoolRepository;
use crate::state::PoolState;
use crate::types::{Amount, PoolType};

/// PancakeSwap V3 计算器（初版）。
#[derive(Clone)]
pub struct PancakeV3SwapCalculator {
    repository: Arc<dyn PoolRepository>,
}

impl PancakeV3SwapCalculator {
    pub fn new(repository: Arc<dyn PoolRepository>) -> Self {
        Self { repository }
    }

    async fn load_state(
        &self,
        pool_id: &crate::types::PoolIdentifier,
    ) -> Result<PancakeV3PoolState, SimulationError> {
        let snapshot = self
            .repository
            .get(pool_id)
            .await
            .map_err(SimulationError::State)?
            .ok_or(SimulationError::State(crate::state::StateError::NotFound))?;
        PancakeV3PoolState::from_snapshot(&snapshot).map_err(SimulationError::State)
    }
}

#[async_trait]
impl SwapCalculator for PancakeV3SwapCalculator {
    async fn simulate(
        &self,
        request: SwapSimulationRequest,
    ) -> Result<SwapSimulationResult, SimulationError> {
        if request.path.is_empty() {
            return Err(SimulationError::InvalidPath);
        }
        let mut amount_in = request.amount_in.0;
        let mut total_fees: u128 = 0;
        let mut snapshots = Vec::new();
        let slippage_bps: u64 = 0;
        let executed_path = request.path.clone();

        for leg in &request.path {
            if leg.pool.pool_type != PoolType::PancakeV3 {
                return Err(SimulationError::Calculation(
                    "路径包含非 Pancake V3 池子".into(),
                ));
            }
            let state = self.load_state(&leg.pool).await?;
            let fee = leg.fee.unwrap_or(100); // 默认 1bp
            let (amount_out, fee_paid) =
                quote_v3_swap(&state, &leg.input, &leg.output, amount_in, fee)?;
            total_fees = total_fees
                .checked_add(fee_paid)
                .ok_or_else(|| SimulationError::Calculation("累计手续费溢出".into()))?;
            amount_in = amount_out;
            snapshots.push(state.snapshot());
        }

        if let Some(min_out) = request.min_amount_out {
            if amount_in < min_out.0 {
                return Err(SimulationError::Calculation(format!(
                    "模拟输出 {} 低于最小要求 {}",
                    amount_in, min_out.0
                )));
            }
        }

        Ok(SwapSimulationResult {
            amount_out: Amount(amount_in),
            slippage_bps,
            fees_paid: Amount(total_fees),
            snapshots,
            executed_path,
            metrics: Default::default(),
        })
    }
}

fn quote_v3_swap(
    state: &PancakeV3PoolState,
    input: &crate::types::Asset,
    output: &crate::types::Asset,
    amount_in: u128,
    fee_bps: u32,
) -> Result<(u128, u128), SimulationError> {
    let sqrt_price = state.sqrt_price_x96();
    let price_x192 = U256::from(sqrt_price) * U256::from(sqrt_price);
    let q192 = U256::one() << 192; // 2^192
    let price = price_x192
        .checked_div(q192)
        .ok_or_else(|| SimulationError::Calculation("价格计算错误".into()))?;

    let fee = fee_bps as u128;
    let fee_denom = 10_000u128;
    let fee_paid = amount_in * fee / fee_denom;
    let amount_after_fee = amount_in - fee_paid;

    if input.address == state.token0().address && output.address == state.token1().address {
        // token0 -> token1
        let amount_out = U256::from(amount_after_fee)
            .checked_mul(price_x192)
            .and_then(|val| val.checked_div(q192))
            .ok_or_else(|| SimulationError::Calculation("输出溢出".into()))?
            .as_u128();
        Ok((amount_out, fee_paid))
    } else if input.address == state.token1().address && output.address == state.token0().address {
        // token1 -> token0
        if price.is_zero() {
            return Err(SimulationError::Calculation("价格为 0".into()));
        }
        if price_x192.is_zero() {
            return Err(SimulationError::Calculation("价格为 0".into()));
        }
        let amount_out = U256::from(amount_after_fee)
            .checked_mul(q192)
            .and_then(|val| val.checked_div(price_x192))
            .ok_or_else(|| SimulationError::Calculation("输出溢出".into()))?
            .as_u128();
        Ok((amount_out, fee_paid))
    } else {
        Err(SimulationError::Calculation("路径资产与池子不匹配".into()))
    }
}
