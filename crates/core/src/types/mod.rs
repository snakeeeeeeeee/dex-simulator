use std::fmt;

use ethers::types::{Address, H256, U256};
use serde::{Deserialize, Serialize};

/// 资产信息描述。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Asset {
    pub address: Address,
    pub symbol: String,
    pub decimals: u8,
}

/// 金额类型，内部使用 u128 表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount(pub u128);

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 池子类别区分，便于不同版本策略。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolType {
    PancakeV2,
    PancakeV3,
    PancakeV4,
    UniswapV2,
    UniswapV3,
    Other(String),
}

/// 池子唯一标识，由链 id、合约地址、DEX 名称组成。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PoolIdentifier {
    pub chain_id: u64,
    pub dex: String,
    pub address: Address,
    pub pool_type: PoolType,
}

/// 单跳路径描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathLeg {
    pub pool: PoolIdentifier,
    pub input: Asset,
    pub output: Asset,
    pub fee: Option<u32>,
}

/// 事件定位帮助结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPointer {
    pub tx_hash: H256,
    pub log_index: u64,
    pub block_number: u64,
}

/// KV 型元数据，便于扩展。
pub type Metadata = std::collections::HashMap<String, String>;

/// 与 ethers 兼容的数量转换，默认对齐到 18 位精度。
pub fn amount_to_u256(amount: Amount, decimals: u8) -> U256 {
    if decimals >= 18 {
        return U256::from(amount.0);
    }
    let diff = 18u32.saturating_sub(decimals as u32) as usize;
    U256::from(amount.0) * U256::exp10(diff)
}
