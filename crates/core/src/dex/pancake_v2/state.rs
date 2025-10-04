use async_trait::async_trait;
use ethers::types::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::dex::pancake_v2::event::{PancakeV2Event, PancakeV2EventError};
use crate::event::EventEnvelope;
use crate::state::{PoolSnapshot, Reserves, StateError};
use crate::types::{Asset, ChainNamespace, PoolIdentifier, PoolType};

/// PancakeSwap V2 池子快照额外信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PancakeV2SnapshotExtra {
    pub token0: Asset,
    pub token1: Asset,
    #[serde(with = "crate::serde_utils::u128_string")]
    pub reserve0: u128,
    #[serde(with = "crate::serde_utils::u128_string")]
    pub reserve1: u128,
    #[serde(with = "crate::serde_utils::option_u128_string")]
    pub total_supply: Option<u128>,
}

/// PancakeSwap V2 池子状态。
#[derive(Debug, Clone)]
pub struct PancakeV2PoolState {
    id: PoolIdentifier,
    token0: Asset,
    token1: Asset,
    reserve0: u128,
    reserve1: u128,
    total_supply: Option<u128>,
    reserves: Reserves,
}

impl PancakeV2PoolState {
    pub fn new(id: PoolIdentifier, token0: Asset, token1: Asset) -> Self {
        let mut id = id;
        id.dex = PoolType::PancakeV2.label();
        id.chain_namespace = ChainNamespace::Evm;
        let mut state = Self {
            id,
            token0,
            token1,
            reserve0: 0,
            reserve1: 0,
            total_supply: None,
            reserves: Reserves::new(),
        };
        state.refresh_reserves();
        state
    }

    pub fn from_snapshot(snapshot: &PoolSnapshot) -> Result<Self, StateError> {
        let extra: PancakeV2SnapshotExtra = serde_yaml::from_value(snapshot.extra.clone())
            .map_err(|err| StateError::Serialize(err.to_string()))?;
        let mut id = snapshot.id.clone();
        id.dex = id.pool_type.label();
        id.chain_namespace = ChainNamespace::Evm;
        let mut state = Self {
            id,
            token0: extra.token0,
            token1: extra.token1,
            reserve0: extra.reserve0,
            reserve1: extra.reserve1,
            total_supply: extra.total_supply,
            reserves: snapshot.reserves.clone(),
        };
        state.refresh_reserves();
        Ok(state)
    }

    pub fn to_snapshot(&self) -> Result<PoolSnapshot, StateError> {
        let extra = PancakeV2SnapshotExtra {
            token0: self.token0.clone(),
            token1: self.token1.clone(),
            reserve0: self.reserve0,
            reserve1: self.reserve1,
            total_supply: self.total_supply,
        };
        let extra_value =
            serde_yaml::to_value(extra).map_err(|err| StateError::Serialize(err.to_string()))?;
        Ok(PoolSnapshot {
            id: self.id.clone(),
            reserves: self.reserves.clone(),
            extra: extra_value,
        })
    }

    pub fn token0(&self) -> &Asset {
        &self.token0
    }

    pub fn token1(&self) -> &Asset {
        &self.token1
    }

    pub fn reserve0(&self) -> u128 {
        self.reserve0
    }

    pub fn reserve1(&self) -> u128 {
        self.reserve1
    }

    pub fn set_reserves(&mut self, reserve0: u128, reserve1: u128) {
        self.reserve0 = reserve0;
        self.reserve1 = reserve1;
        self.refresh_reserves();
    }

    pub fn set_total_supply(&mut self, total_supply: Option<u128>) {
        self.total_supply = total_supply;
    }

    pub fn apply_pancake_event(&mut self, event: &PancakeV2Event) -> Result<(), StateError> {
        match event {
            PancakeV2Event::PairCreated { .. } => Ok(()),
            PancakeV2Event::Mint {
                pair,
                amount0,
                amount1,
                ..
            } => {
                self.ensure_pair(pair)?;
                let a0 = u256_to_u128(amount0)?;
                let a1 = u256_to_u128(amount1)?;
                self.reserve0 = self
                    .reserve0
                    .checked_add(a0)
                    .ok_or_else(|| StateError::UpdateFailed("reserve0 溢出".into()))?;
                self.reserve1 = self
                    .reserve1
                    .checked_add(a1)
                    .ok_or_else(|| StateError::UpdateFailed("reserve1 溢出".into()))?;
                self.refresh_reserves();
                Ok(())
            }
            PancakeV2Event::Burn {
                pair,
                amount0,
                amount1,
                ..
            } => {
                self.ensure_pair(pair)?;
                let a0 = u256_to_u128(amount0)?;
                let a1 = u256_to_u128(amount1)?;
                self.reserve0 = self
                    .reserve0
                    .checked_sub(a0)
                    .ok_or_else(|| StateError::UpdateFailed("reserve0 不足".into()))?;
                self.reserve1 = self
                    .reserve1
                    .checked_sub(a1)
                    .ok_or_else(|| StateError::UpdateFailed("reserve1 不足".into()))?;
                self.refresh_reserves();
                Ok(())
            }
            PancakeV2Event::Swap {
                pair,
                amount0_in,
                amount1_in,
                amount0_out,
                amount1_out,
                ..
            } => {
                self.ensure_pair(pair)?;
                let a0_in = u256_to_u128(amount0_in)?;
                let a1_in = u256_to_u128(amount1_in)?;
                let a0_out = u256_to_u128(amount0_out)?;
                let a1_out = u256_to_u128(amount1_out)?;
                self.reserve0 = self
                    .reserve0
                    .checked_add(a0_in)
                    .and_then(|val| val.checked_sub(a0_out))
                    .ok_or_else(|| StateError::UpdateFailed("reserve0 计算失败".into()))?;
                self.reserve1 = self
                    .reserve1
                    .checked_add(a1_in)
                    .and_then(|val| val.checked_sub(a1_out))
                    .ok_or_else(|| StateError::UpdateFailed("reserve1 计算失败".into()))?;
                self.refresh_reserves();
                Ok(())
            }
            PancakeV2Event::Sync {
                pair,
                reserve0,
                reserve1,
            } => {
                self.ensure_pair(pair)?;
                self.reserve0 = u256_to_u128(reserve0)?;
                self.reserve1 = u256_to_u128(reserve1)?;
                self.refresh_reserves();
                Ok(())
            }
        }
    }

    pub fn quote_swap(
        &self,
        input_asset: &Asset,
        output_asset: &Asset,
        amount_in: u128,
        fee_bps: u32,
    ) -> Result<(u128, u128, PancakeV2PoolState), StateError> {
        if amount_in == 0 {
            return Err(StateError::Invalid("amount_in 不能为 0".into()));
        }
        let fee_denom = 10_000u128;
        let fee_num = fee_denom
            .checked_sub(fee_bps as u128)
            .ok_or_else(|| StateError::Invalid("手续费配置异常".into()))?;
        let (reserve_in, reserve_out, direction) = if input_asset.address == self.token0.address
            && output_asset.address == self.token1.address
        {
            (self.reserve0, self.reserve1, SwapDirection::ZeroToOne)
        } else if input_asset.address == self.token1.address
            && output_asset.address == self.token0.address
        {
            (self.reserve1, self.reserve0, SwapDirection::OneToZero)
        } else {
            return Err(StateError::Invalid("输入输出资产与池子不匹配".into()));
        };
        if reserve_in == 0 || reserve_out == 0 {
            return Err(StateError::Invalid("池子储备不足".into()));
        }
        let amount_in_u256 = U256::from(amount_in);
        let reserve_in_u256 = U256::from(reserve_in);
        let reserve_out_u256 = U256::from(reserve_out);
        let fee_num_u256 = U256::from(fee_num);
        let fee_denom_u256 = U256::from(fee_denom);
        let amount_in_with_fee = amount_in_u256 * fee_num_u256;
        let numerator = amount_in_with_fee * reserve_out_u256;
        let denominator = reserve_in_u256 * fee_denom_u256 + amount_in_with_fee;
        if denominator.is_zero() {
            return Err(StateError::Invalid("swap 除数为 0".into()));
        }
        let amount_out_u256 = numerator / denominator;
        if amount_out_u256.is_zero() {
            return Err(StateError::Invalid("输出数量为 0".into()));
        }
        let amount_out = u256_to_u128(&amount_out_u256)?;
        let mut next_state = self.clone();
        match direction {
            SwapDirection::ZeroToOne => {
                next_state.reserve0 = next_state
                    .reserve0
                    .checked_add(amount_in)
                    .ok_or_else(|| StateError::UpdateFailed("reserve0 更新失败".into()))?;
                next_state.reserve1 = next_state
                    .reserve1
                    .checked_sub(amount_out)
                    .ok_or_else(|| StateError::UpdateFailed("reserve1 更新失败".into()))?;
            }
            SwapDirection::OneToZero => {
                next_state.reserve1 = next_state
                    .reserve1
                    .checked_add(amount_in)
                    .ok_or_else(|| StateError::UpdateFailed("reserve1 更新失败".into()))?;
                next_state.reserve0 = next_state
                    .reserve0
                    .checked_sub(amount_out)
                    .ok_or_else(|| StateError::UpdateFailed("reserve0 更新失败".into()))?;
            }
        }
        next_state.refresh_reserves();

        let fee_paid = amount_in.saturating_mul(fee_bps as u128) / fee_denom;
        Ok((amount_out, fee_paid, next_state))
    }

    fn ensure_pair(&self, pair: &Address) -> Result<(), StateError> {
        if &self.id.address == pair {
            Ok(())
        } else {
            Err(StateError::Invalid("事件地址与池子不匹配".into()))
        }
    }

    fn refresh_reserves(&mut self) {
        self.reserves.set(self.token0.clone(), self.reserve0);
        self.reserves.set(self.token1.clone(), self.reserve1);
    }
}

#[async_trait]
impl crate::state::PoolState for PancakeV2PoolState {
    fn id(&self) -> &PoolIdentifier {
        &self.id
    }

    fn reserves(&self) -> &Reserves {
        &self.reserves
    }

    async fn apply_event(&mut self, event: &EventEnvelope) -> Result<(), StateError> {
        match PancakeV2Event::try_from(event) {
            Ok(parsed) => self.apply_pancake_event(&parsed),
            Err(PancakeV2EventError::Unsupported) => Ok(()),
            Err(err) => Err(StateError::UpdateFailed(err.to_string())),
        }
    }

    fn snapshot(&self) -> PoolSnapshot {
        self.to_snapshot().expect("snapshot 转换失败")
    }
}

#[derive(Debug, Clone, Copy)]
enum SwapDirection {
    ZeroToOne,
    OneToZero,
}

fn u256_to_u128(value: &U256) -> Result<u128, StateError> {
    if *value > U256::from(u128::MAX) {
        return Err(StateError::UpdateFailed("数值超过 u128 范围".into()));
    }
    Ok(value.as_u128())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PoolState;
    use ethers::abi::{encode, Token};
    use ethers::types::{Bytes, H256};

    fn sample_assets() -> (Asset, Asset) {
        let token0 = Asset {
            address: addr(0x11),
            symbol: "T0".into(),
            decimals: 18,
        };
        let token1 = Asset {
            address: addr(0x22),
            symbol: "T1".into(),
            decimals: 18,
        };
        (token0, token1)
    }

    fn sample_pool_state(reserve0: u128, reserve1: u128) -> PancakeV2PoolState {
        let (token0, token1) = sample_assets();
        let pool_type = crate::types::PoolType::PancakeV2;
        let id = PoolIdentifier {
            chain_namespace: crate::types::ChainNamespace::Evm,
            chain_id: 56,
            dex: pool_type.label(),
            address: addr(0x33),
            pool_type,
        };
        let mut state = PancakeV2PoolState::new(id, token0, token1);
        state.set_reserves(reserve0, reserve1);
        state
    }

    #[tokio::test]
    async fn test_apply_swap_event() {
        let mut state = sample_pool_state(1_000_000, 500_000);
        let event = EventEnvelope {
            kind: crate::event::EventKind::Swap,
            block_number: 1,
            transaction_hash: H256::zero(),
            log_index: 0,
            address: state.id().address,
            topics: vec![
                crate::dex::pancake_v2::swap_topic(),
                topic(state.token0().address),
                topic(state.token1().address),
            ],
            payload: Bytes::from(encode(&[
                Token::Uint(U256::from(100u64)),
                Token::Uint(U256::zero()),
                Token::Uint(U256::zero()),
                Token::Uint(U256::from(45u64)),
            ])),
        };
        state.apply_event(&event).await.expect("事件应用失败");
        assert_eq!(state.reserve0(), 1_000_100);
        assert_eq!(state.reserve1(), 499_955);
    }

    #[test]
    fn test_quote_swap() {
        let state = sample_pool_state(1_000_000_000_000u128, 500_000_000_000u128);
        let (amount_out, fee, next_state) = state
            .quote_swap(state.token0(), state.token1(), 100_000_000, 25)
            .expect("quote 失败");
        assert!(amount_out > 0);
        assert!(fee > 0);
        // 验证恒定乘积关系未被破坏（考虑手续费后允许小范围误差）。
        let k_before = state.reserve0() as u128 * state.reserve1() as u128;
        let k_after = next_state.reserve0() as u128 * next_state.reserve1() as u128;
        assert!(k_after >= k_before);
    }

    fn topic(addr: Address) -> H256 {
        let mut bytes = [0u8; 32];
        bytes[12..].copy_from_slice(addr.as_bytes());
        H256::from(bytes)
    }

    fn addr(byte: u8) -> Address {
        let mut bytes = [0u8; 20];
        bytes.fill(byte);
        Address::from_slice(&bytes)
    }
}
