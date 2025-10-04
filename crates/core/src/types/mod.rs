use std::fmt;

use ethers::types::{Address, H256, U256};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::serde_utils;

/// 资产信息描述。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Asset {
    pub address: Address,
    pub symbol: String,
    pub decimals: u8,
}

/// 金额类型，内部使用 u128 表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount(pub u128);

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Amount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_utils::u128_string::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_utils::u128_string::deserialize(deserializer).map(Amount)
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

impl PoolType {
    /// 返回带版本的 DEX 标签字符串。
    pub fn label(&self) -> String {
        match self {
            PoolType::PancakeV2 => "pancake_v2".into(),
            PoolType::PancakeV3 => "pancake_v3".into(),
            PoolType::PancakeV4 => "pancake_v4".into(),
            PoolType::UniswapV2 => "uniswap_v2".into(),
            PoolType::UniswapV3 => "uniswap_v3".into(),
            PoolType::Other(name) => name.clone(),
        }
    }
}

/// 链类别，方便跨链扩展。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChainNamespace {
    Evm,
    Solana,
    Other(String),
}

impl Default for ChainNamespace {
    fn default() -> Self {
        ChainNamespace::Evm
    }
}

/// 池子唯一标识，由链 id、合约地址、DEX 名称组成。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PoolIdentifier {
    #[serde(default)]
    pub chain_namespace: ChainNamespace,
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
