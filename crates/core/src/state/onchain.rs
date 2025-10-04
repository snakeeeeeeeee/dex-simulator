use std::convert::TryInto;
use std::sync::Arc;

use ethers::contract::abigen;
use ethers::providers::Middleware;
use ethers::types::{Address, U256};
use thiserror::Error;

use crate::dex::bootstrap::{V2BootstrapPool, V3BootstrapPool};
use crate::dex::pancake_v2::state::PancakeV2PoolState;
use crate::dex::pancake_v3::state::PancakeV3PoolState;
use crate::state::{PoolSnapshot, PoolState};
use crate::types::{PoolIdentifier, PoolType};

abigen!(
    PancakeV2Pair,
    r#"[
        function token0() view returns (address)
        function token1() view returns (address)
        function getReserves() view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast)
        function totalSupply() view returns (uint256)
    ]"#
);

abigen!(
    PancakeV3PoolContract,
    r#"[
        function token0() view returns (address)
        function token1() view returns (address)
        function slot0() view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)
        function liquidity() view returns (uint128)
    ]"#
);

abigen!(
    Erc20,
    r#"[
        function balanceOf(address) view returns (uint256)
    ]"#
);

#[derive(Debug, Clone)]
pub struct OnChainStateFetcher<M: Middleware> {
    provider: Arc<M>,
    chain_id: u64,
    dex: String,
}

impl<M: Middleware> OnChainStateFetcher<M> {
    pub fn new(provider: Arc<M>, chain_id: u64, dex: impl Into<String>) -> Self {
        Self {
            provider,
            chain_id,
            dex: dex.into(),
        }
    }

    fn pool_identifier(&self, address: Address, pool_type: PoolType) -> PoolIdentifier {
        PoolIdentifier {
            chain_id: self.chain_id,
            dex: self.dex.clone(),
            address,
            pool_type,
        }
    }

    pub async fn fetch_v2_snapshot(
        &self,
        pool: &V2BootstrapPool,
    ) -> Result<PoolSnapshot, OnChainSnapshotError> {
        let pair = PancakeV2Pair::new(pool.pool, self.provider.clone());
        let reserves = pair
            .get_reserves()
            .call()
            .await
            .map_err(|err| OnChainSnapshotError::Rpc(err.to_string()))?;
        let total_supply = match pair.total_supply().call().await {
            Ok(supply) => Some(u256_to_u128(supply)?),
            Err(err) => {
                log::warn!(
                    "获取 V2 totalSupply 失败: pool={:#x}, 错误: {}",
                    pool.pool,
                    err
                );
                None
            }
        };

        let mut state = PancakeV2PoolState::new(
            self.pool_identifier(pool.pool, PoolType::PancakeV2),
            pool.token0.clone(),
            pool.token1.clone(),
        );
        let reserve0: u128 = reserves.0.into();
        let reserve1: u128 = reserves.1.into();
        state.set_reserves(reserve0, reserve1);
        state.set_total_supply(total_supply);
        state
            .to_snapshot()
            .map_err(|err| OnChainSnapshotError::State(err.to_string()))
    }

    pub async fn fetch_v3_snapshot(
        &self,
        pool: &V3BootstrapPool,
    ) -> Result<PoolSnapshot, OnChainSnapshotError> {
        let contract = PancakeV3PoolContract::new(pool.pool, self.provider.clone());
        let slot0 = contract
            .slot_0()
            .call()
            .await
            .map_err(|err| OnChainSnapshotError::Rpc(err.to_string()))?;
        let liquidity = contract
            .liquidity()
            .call()
            .await
            .map_err(|err| OnChainSnapshotError::Rpc(err.to_string()))?;

        let token0_balance = Erc20::new(pool.token0.address, self.provider.clone())
            .balance_of(pool.pool)
            .call()
            .await
            .map_err(|err| OnChainSnapshotError::Rpc(err.to_string()))?;
        let token1_balance = Erc20::new(pool.token1.address, self.provider.clone())
            .balance_of(pool.pool)
            .call()
            .await
            .map_err(|err| OnChainSnapshotError::Rpc(err.to_string()))?;

        let mut state = PancakeV3PoolState::new(
            self.pool_identifier(pool.pool, PoolType::PancakeV3),
            pool.token0.clone(),
            pool.token1.clone(),
        );
        state.set_price_liquidity(slot0.0.into(), liquidity.into(), slot0.1);
        state.set_reserves(u256_to_u128(token0_balance)?, u256_to_u128(token1_balance)?);
        Ok(state.snapshot())
    }
}

#[derive(Error, Debug)]
pub enum OnChainSnapshotError {
    #[error("RPC 调用失败: {0}")]
    Rpc(String),
    #[error("状态错误: {0}")]
    State(String),
    #[error("数值转换失败: {0}")]
    Conversion(String),
}

fn u256_to_u128(value: U256) -> Result<u128, OnChainSnapshotError> {
    value
        .try_into()
        .map_err(|_| OnChainSnapshotError::Conversion("数值超过 u128".into()))
}
