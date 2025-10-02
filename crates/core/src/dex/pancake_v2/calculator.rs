use std::sync::Arc;

use async_trait::async_trait;

use crate::dex::pancake_v2::state::PancakeV2PoolState;
use crate::simulation::{
    SimulationError, SwapCalculator, SwapSimulationRequest, SwapSimulationResult,
};
use crate::state::repository::PoolRepository;
use crate::state::StateError;
use crate::types::{Amount, PoolIdentifier};
use ethers::types::U256;

/// PancakeSwap V2 Swap 计算器。
#[derive(Clone)]
pub struct PancakeV2SwapCalculator {
    repository: Arc<dyn PoolRepository>,
    fee_bps: u32,
}

impl PancakeV2SwapCalculator {
    pub fn new(repository: Arc<dyn PoolRepository>, fee_bps: u32) -> Self {
        Self {
            repository,
            fee_bps,
        }
    }

    fn select_fee(&self, leg_fee: Option<u32>) -> u32 {
        leg_fee.unwrap_or(self.fee_bps)
    }

    async fn load_state(&self, id: &PoolIdentifier) -> Result<PancakeV2PoolState, SimulationError> {
        let snapshot = self
            .repository
            .get(id)
            .await
            .map_err(SimulationError::State)?
            .ok_or_else(|| SimulationError::State(StateError::NotFound))?;
        PancakeV2PoolState::from_snapshot(&snapshot).map_err(SimulationError::State)
    }
}

#[async_trait]
impl SwapCalculator for PancakeV2SwapCalculator {
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
        let mut slippage_bps: u64 = 0;
        let executed_path = request.path.clone();
        let mut step_metrics: Vec<crate::simulation::StepMetric> = Vec::new();

        for leg in &request.path {
            let step_start = std::time::Instant::now();
            if leg.pool.pool_type != crate::types::PoolType::PancakeV2 {
                return Err(SimulationError::Calculation(
                    "路径中包含非 Pancake V2 池子".into(),
                ));
            }
            let fee_bps = self.select_fee(leg.fee);
            let state = self.load_state(&leg.pool).await?;
            let (amount_out, fee_paid, next_state) =
                state.quote_swap(&leg.input, &leg.output, amount_in, fee_bps)?;
            total_fees = total_fees
                .checked_add(fee_paid)
                .ok_or_else(|| SimulationError::Calculation("累计手续费溢出".into()))?;
            let (reserve_in, reserve_out) = if leg.input.address == state.token0().address
                && leg.output.address == state.token1().address
            {
                (state.reserve0(), state.reserve1())
            } else if leg.input.address == state.token1().address
                && leg.output.address == state.token0().address
            {
                (state.reserve1(), state.reserve0())
            } else {
                return Err(SimulationError::Calculation("路径资产与池子不匹配".into()));
            };
            if reserve_in == 0 || reserve_out == 0 {
                return Err(SimulationError::Calculation("池子储备不足".into()));
            }
            let expected_out =
                (U256::from(amount_in) * U256::from(reserve_out)) / U256::from(reserve_in);
            let actual_out = U256::from(amount_out);
            if expected_out > actual_out && !expected_out.is_zero() {
                let diff = expected_out - actual_out;
                let slip = diff * U256::from(10_000u64) / expected_out;
                let slip_u64 = slip.min(U256::from(u64::MAX)).as_u64();
                slippage_bps = slippage_bps.saturating_add(slip_u64);
            }
            amount_in = amount_out;
            snapshots.push(next_state.to_snapshot().map_err(SimulationError::State)?);
            let step_duration = step_start.elapsed().as_secs_f64() * 1000.0;
            step_metrics.push(crate::simulation::StepMetric {
                name: format!("{}->{}", leg.input.symbol, leg.output.symbol),
                duration_ms: step_duration,
            });
        }

        if let Some(min_out) = request.min_amount_out {
            if amount_in < min_out.0 {
                return Err(SimulationError::Calculation(format!(
                    "模拟输出 {} 低于最小要求 {}",
                    amount_in, min_out.0
                )));
            }
        }

        let total_duration: f64 = step_metrics.iter().map(|m| m.duration_ms).sum();

        Ok(SwapSimulationResult {
            amount_out: Amount(amount_in),
            slippage_bps,
            fees_paid: Amount(total_fees),
            snapshots,
            executed_path,
            metrics: crate::simulation::SimulationMetrics {
                calc_duration_ms: total_duration,
                steps: step_metrics,
            },
        })
    }
}
