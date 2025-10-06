use std::collections::HashMap;
use std::convert::TryInto;

use async_trait::async_trait;
use ethers::types::{I256, U256};
use serde::{Deserialize, Serialize};
use serde_yaml;

use crate::dex::pancake_v3::event::PancakeV3Event;
use crate::dex::traits::ConcentratedLiquidityPoolState;
use crate::event::EventEnvelope;
use crate::state::{PoolSnapshot, PoolState, Reserves, StateError};
use crate::types::{Asset, ChainNamespace, PoolIdentifier, PoolType};

/// V3 Tick 信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TickInfo {
    pub liquidity_net: U256,
    pub liquidity_gross: U256,
}

/// PancakeSwap V3 池子状态。
#[derive(Debug, Clone)]
pub struct PancakeV3PoolState {
    id: PoolIdentifier,
    token0: Asset,
    token1: Asset,
    sqrt_price_x96: U256,
    liquidity: U256,
    tick: i32,
    reserves: Reserves,
    ticks: HashMap<i32, TickInfo>,
    fees_token0: U256,
    fees_token1: U256,
}

impl PancakeV3PoolState {
    pub fn new(mut id: PoolIdentifier, token0: Asset, token1: Asset) -> Self {
        id.dex = PoolType::PancakeV3.label();
        id.chain_namespace = ChainNamespace::Evm;
        let mut reserves = Reserves::new();
        reserves.set(token0.clone(), 0);
        reserves.set(token1.clone(), 0);
        Self {
            id,
            token0,
            token1,
            sqrt_price_x96: U256::zero(),
            liquidity: U256::zero(),
            tick: 0,
            reserves,
            ticks: HashMap::new(),
            fees_token0: U256::zero(),
            fees_token1: U256::zero(),
        }
    }

    pub fn token0(&self) -> &Asset {
        &self.token0
    }

    pub fn token1(&self) -> &Asset {
        &self.token1
    }

    pub fn set_price_liquidity(&mut self, sqrt_price_x96: U256, liquidity: U256, tick: i32) {
        self.sqrt_price_x96 = sqrt_price_x96;
        self.liquidity = liquidity;
        self.tick = tick;
    }

    pub fn set_reserves(&mut self, amount0: u128, amount1: u128) {
        self.reserves.set(self.token0.clone(), amount0);
        self.reserves.set(self.token1.clone(), amount1);
    }

    pub fn from_snapshot(snapshot: &PoolSnapshot) -> Result<Self, StateError> {
        let extra: PancakeV3Snapshot = serde_yaml::from_value(snapshot.extra.clone())
            .map_err(|err| StateError::Serialize(err.to_string()))?;
        let mut id = snapshot.id.clone();
        id.dex = id.pool_type.label();
        id.chain_namespace = ChainNamespace::Evm;
        Ok(Self {
            id,
            token0: extra.token0,
            token1: extra.token1,
            sqrt_price_x96: extra.sqrt_price_x96,
            liquidity: extra.liquidity,
            tick: extra.tick,
            reserves: snapshot.reserves.clone(),
            ticks: extra.ticks,
            fees_token0: extra.fees_token0,
            fees_token1: extra.fees_token1,
        })
    }

    pub fn update_from_event(&mut self, event: &PancakeV3Event) -> Result<(), StateError> {
        match event {
            PancakeV3Event::Initialize {
                sqrt_price_x96,
                tick,
            } => {
                self.sqrt_price_x96 = *sqrt_price_x96;
                self.tick = *tick;
                Ok(())
            }
            PancakeV3Event::Mint {
                liquidity,
                amount0,
                amount1,
                tick_lower,
                tick_upper,
                ..
            } => {
                self.liquidity += *liquidity;
                self.update_tick(*tick_lower, *liquidity, true);
                self.update_tick(*tick_upper, *liquidity, false);
                self.add_reserve(*amount0, *amount1)?;
                Ok(())
            }
            PancakeV3Event::Burn {
                liquidity,
                amount0,
                amount1,
                tick_lower,
                tick_upper,
                ..
            } => {
                if self.liquidity < *liquidity {
                    return Err(StateError::UpdateFailed("流动性不足".into()));
                }
                self.liquidity -= *liquidity;
                self.update_tick_burn(*tick_lower, *liquidity, true);
                self.update_tick_burn(*tick_upper, *liquidity, false);
                self.sub_reserve(*amount0, *amount1)?;
                Ok(())
            }
            PancakeV3Event::Swap {
                amount0,
                amount1,
                sqrt_price_x96,
                liquidity,
                tick,
                ..
            } => {
                self.adjust_reserve_signed(*amount0, *amount1)?;
                self.sqrt_price_x96 = *sqrt_price_x96;
                self.liquidity = *liquidity;
                self.tick = *tick;
                Ok(())
            }
            PancakeV3Event::Collect {
                amount0, amount1, ..
            } => {
                if let Err(err) = self.sub_reserve(*amount0, *amount1) {
                    tracing::warn!(
                        "Collect 扣减失败: pool={:#x}, 错误: {}",
                        self.id.address,
                        err
                    );
                } else {
                    self.fees_token0 += *amount0;
                    self.fees_token1 += *amount1;
                }
                Ok(())
            }
            PancakeV3Event::Unknown => Ok(()),
        }
    }

    fn update_tick(&mut self, tick: i32, liquidity: U256, is_lower: bool) {
        let entry = self.ticks.entry(tick).or_default();
        entry.liquidity_gross += liquidity;
        if is_lower {
            entry.liquidity_net += liquidity;
        } else {
            entry.liquidity_net = entry.liquidity_net.saturating_sub(liquidity);
        }
    }

    fn update_tick_burn(&mut self, tick: i32, liquidity: U256, is_lower: bool) {
        if let Some(entry) = self.ticks.get_mut(&tick) {
            entry.liquidity_gross = entry.liquidity_gross.saturating_sub(liquidity);
            if is_lower {
                entry.liquidity_net = entry.liquidity_net.saturating_sub(liquidity);
            } else {
                entry.liquidity_net += liquidity;
            }
        }
    }

    fn add_reserve(&mut self, amount0: U256, amount1: U256) -> Result<(), StateError> {
        let delta0 = u256_to_u128(amount0)?;
        let delta1 = u256_to_u128(amount1)?;
        if let Some(reserve0) = self.reserves.amounts.get_mut(&self.token0) {
            *reserve0 = reserve0.saturating_add(delta0);
        }
        if let Some(reserve1) = self.reserves.amounts.get_mut(&self.token1) {
            *reserve1 = reserve1.saturating_add(delta1);
        }
        Ok(())
    }

    fn sub_reserve(&mut self, amount0: U256, amount1: U256) -> Result<(), StateError> {
        let delta0 = u256_to_u128(amount0)?;
        let delta1 = u256_to_u128(amount1)?;
        if let Some(reserve0) = self.reserves.amounts.get_mut(&self.token0) {
            *reserve0 = reserve0
                .checked_sub(delta0)
                .ok_or_else(|| StateError::UpdateFailed("token0 储备不足".into()))?;
        }
        if let Some(reserve1) = self.reserves.amounts.get_mut(&self.token1) {
            *reserve1 = reserve1
                .checked_sub(delta1)
                .ok_or_else(|| StateError::UpdateFailed("token1 储备不足".into()))?;
        }
        Ok(())
    }

    fn adjust_reserve_signed(&mut self, amount0: I256, amount1: I256) -> Result<(), StateError> {
        let token0 = self.token0.clone();
        if let Some(reserve0) = self.reserves.amounts.get_mut(&token0) {
            *reserve0 = if amount0.is_negative() {
                let delta = u256_to_u128(amount0.unsigned_abs())?;
                reserve0
                    .checked_sub(delta)
                    .ok_or_else(|| StateError::UpdateFailed("token0 储备不足".into()))?
            } else {
                reserve0.saturating_add(i256_to_u128(amount0)?)
            };
        }

        let token1 = self.token1.clone();
        if let Some(reserve1) = self.reserves.amounts.get_mut(&token1) {
            *reserve1 = if amount1.is_negative() {
                let delta = u256_to_u128(amount1.unsigned_abs())?;
                reserve1
                    .checked_sub(delta)
                    .ok_or_else(|| StateError::UpdateFailed("token1 储备不足".into()))?
            } else {
                reserve1.saturating_add(i256_to_u128(amount1)?)
            };
        }
        Ok(())
    }
}

#[async_trait]
impl PoolState for PancakeV3PoolState {
    fn id(&self) -> &PoolIdentifier {
        &self.id
    }

    fn reserves(&self) -> &Reserves {
        &self.reserves
    }

    async fn apply_event(&mut self, event: &EventEnvelope) -> Result<(), StateError> {
        match PancakeV3Event::try_from(event) {
            Ok(parsed) => self.update_from_event(&parsed),
            Err(err) => Err(StateError::UpdateFailed(err.to_string())),
        }
    }

    fn snapshot(&self) -> PoolSnapshot {
        let snapshot = PancakeV3Snapshot {
            token0: self.token0.clone(),
            token1: self.token1.clone(),
            sqrt_price_x96: self.sqrt_price_x96,
            liquidity: self.liquidity,
            tick: self.tick,
            ticks: self.ticks.clone(),
            fees_token0: self.fees_token0,
            fees_token1: self.fees_token1,
        };
        let extra = serde_yaml::to_value(&snapshot).expect("snapshot serialize");
        PoolSnapshot {
            id: self.id.clone(),
            reserves: self.reserves.clone(),
            extra,
        }
    }
}

impl ConcentratedLiquidityPoolState for PancakeV3PoolState {
    fn sqrt_price_x96(&self) -> U256 {
        self.sqrt_price_x96
    }

    fn current_tick(&self) -> i32 {
        self.tick
    }

    fn liquidity(&self) -> U256 {
        self.liquidity
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PancakeV3Snapshot {
    pub token0: Asset,
    pub token1: Asset,
    #[serde(with = "crate::serde_utils::u256_string")]
    pub sqrt_price_x96: U256,
    #[serde(with = "crate::serde_utils::u256_string")]
    pub liquidity: U256,
    pub tick: i32,
    pub ticks: HashMap<i32, TickInfo>,
    #[serde(with = "crate::serde_utils::u256_string")]
    pub fees_token0: U256,
    #[serde(with = "crate::serde_utils::u256_string")]
    pub fees_token1: U256,
}

fn u256_to_u128(value: U256) -> Result<u128, StateError> {
    value
        .try_into()
        .map_err(|_| StateError::UpdateFailed("数值超过 u128".into()))
}

fn i256_to_u128(value: I256) -> Result<u128, StateError> {
    if value.is_negative() {
        return Err(StateError::UpdateFailed("负值无法转换为 u128".into()));
    }
    u256_to_u128(value.into_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::Address;

    fn sample_assets() -> (Asset, Asset) {
        let token0 = Asset {
            address: Address::repeat_byte(0x11),
            symbol: "TOKEN0".into(),
            decimals: 18,
        };
        let token1 = Asset {
            address: Address::repeat_byte(0x22),
            symbol: "TOKEN1".into(),
            decimals: 18,
        };
        (token0, token1)
    }

    fn sample_identifier() -> PoolIdentifier {
        let pool_type = crate::types::PoolType::PancakeV3;
        PoolIdentifier {
            chain_namespace: crate::types::ChainNamespace::Evm,
            chain_id: 56,
            dex: pool_type.label(),
            address: Address::repeat_byte(0x33),
            pool_type,
        }
    }

    #[tokio::test]
    async fn test_initialize_and_mint() {
        let (token0, token1) = sample_assets();
        let mut state =
            PancakeV3PoolState::new(sample_identifier(), token0.clone(), token1.clone());
        state
            .update_from_event(&PancakeV3Event::Initialize {
                sqrt_price_x96: U256::from(1_000u64),
                tick: 10,
            })
            .unwrap();
        assert_eq!(state.sqrt_price_x96(), U256::from(1_000u64));
        assert_eq!(state.current_tick(), 10);

        state
            .update_from_event(&PancakeV3Event::Mint {
                sender: Address::zero(),
                owner: Address::zero(),
                tick_lower: -60,
                tick_upper: 60,
                liquidity: U256::from(1_000u64),
                amount0: U256::from(500u64),
                amount1: U256::from(600u64),
            })
            .unwrap();

        let reserves = state.reserves();
        assert_eq!(reserves.amounts.get(&token0).copied(), Some(500));
        assert_eq!(reserves.amounts.get(&token1).copied(), Some(600));
        assert_eq!(state.liquidity(), U256::from(1_000u64));
    }

    #[tokio::test]
    async fn test_snapshot_roundtrip() {
        let (token0, token1) = sample_assets();
        let mut state = PancakeV3PoolState::new(sample_identifier(), token0, token1);
        state
            .update_from_event(&PancakeV3Event::Initialize {
                sqrt_price_x96: U256::from(2_000u64),
                tick: -15,
            })
            .unwrap();
        state
            .update_from_event(&PancakeV3Event::Mint {
                sender: Address::zero(),
                owner: Address::zero(),
                tick_lower: -120,
                tick_upper: 120,
                liquidity: U256::from(2_000u64),
                amount0: U256::from(300u64),
                amount1: U256::from(400u64),
            })
            .unwrap();
        state
            .update_from_event(&PancakeV3Event::Burn {
                owner: Address::zero(),
                tick_lower: -120,
                tick_upper: 120,
                liquidity: U256::from(500u64),
                amount0: U256::from(50u64),
                amount1: U256::from(60u64),
            })
            .unwrap();

        let snapshot = state.snapshot();
        let restored = PancakeV3PoolState::from_snapshot(&snapshot).unwrap();
        assert_eq!(restored.sqrt_price_x96(), state.sqrt_price_x96());
        assert_eq!(restored.current_tick(), state.current_tick());
        assert_eq!(restored.liquidity(), state.liquidity());
    }
}
