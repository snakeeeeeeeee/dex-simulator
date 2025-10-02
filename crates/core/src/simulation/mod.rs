use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::state::{PoolSnapshot, StateError};
use crate::types::{Amount, PathLeg};

/// 模拟请求参数，描述单次或多跳 swap。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapSimulationRequest {
    pub path: Vec<PathLeg>,
    pub amount_in: Amount,
    pub min_amount_out: Option<Amount>,
    pub sandbox: bool,
}

/// 模拟结果，包含输出、滑点等指标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapSimulationResult {
    pub amount_out: Amount,
    pub slippage_bps: u64,
    pub fees_paid: Amount,
    pub snapshots: Vec<PoolSnapshot>,
    pub executed_path: Vec<PathLeg>,
    pub metrics: SimulationMetrics,
}

/// 模拟过程的性能指标。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SimulationMetrics {
    pub calc_duration_ms: f64,
    pub steps: Vec<StepMetric>,
}

/// 单个步骤耗时。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepMetric {
    pub name: String,
    pub duration_ms: f64,
}

/// Swap 计算器接口，针对不同 DEX/V 版本提供实现。
#[async_trait]
pub trait SwapCalculator: Send + Sync {
    async fn simulate(
        &self,
        request: SwapSimulationRequest,
    ) -> Result<SwapSimulationResult, SimulationError>;
}

/// 占位计算器，阶段 1 用于打通调用链。
#[derive(Default, Debug, Clone)]
pub struct NoopSwapCalculator;

#[async_trait]
impl SwapCalculator for NoopSwapCalculator {
    async fn simulate(
        &self,
        request: SwapSimulationRequest,
    ) -> Result<SwapSimulationResult, SimulationError> {
        if request.path.is_empty() {
            return Err(SimulationError::InvalidPath);
        }
        let executed_path = request.path.clone();
        Ok(SwapSimulationResult {
            amount_out: request.amount_in,
            slippage_bps: 0,
            fees_paid: Amount(0),
            snapshots: Vec::new(),
            executed_path,
            metrics: SimulationMetrics::default(),
        })
    }
}

/// 模拟相关错误。
#[derive(thiserror::Error, Debug)]
pub enum SimulationError {
    #[error("状态错误: {0}")]
    State(#[from] StateError),
    #[error("计算失败: {0}")]
    Calculation(String),
    #[error("路径无效")]
    InvalidPath,
}
