use async_trait::async_trait;
use ethers::types::U256;

use crate::event::{EventEnvelope, EventListenerError};
use crate::state::PoolState;

/// 通用 DEX 模块接口，统一事件处理入口。
#[async_trait]
pub trait DexEventHandler: Send + Sync {
    async fn handle_envelope(&self, envelope: EventEnvelope) -> Result<(), EventListenerError>;

    /// 返回事件是否被当前处理器识别，用于路由模块。
    fn matches(&self, envelope: &EventEnvelope) -> bool;
}

/// 集中流动性池子共性能力抽象。
pub trait ConcentratedLiquidityPoolState: PoolState {
    fn sqrt_price_x96(&self) -> U256;
    fn current_tick(&self) -> i32;
    fn liquidity(&self) -> U256;
}
