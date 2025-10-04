pub mod onchain;
pub mod repository;
pub mod snapshot;

use std::collections::HashMap;

use async_trait::async_trait;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::event::EventEnvelope;
use crate::types::{Asset, PoolIdentifier};

/// 通用储备表示。
#[derive(Debug, Clone, Default)]
pub struct Reserves {
    pub amounts: HashMap<Asset, u128>,
}

impl Reserves {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, asset: Asset, amount: u128) {
        self.amounts.insert(asset, amount);
    }

    pub fn get(&self, asset: &Asset) -> Option<&u128> {
        self.amounts.get(asset)
    }
}

impl Serialize for Reserves {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Reserves", 1)?;
        state.serialize_field(
            "amounts",
            &ReservesAmountSerializer {
                inner: &self.amounts,
            },
        )?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Reserves {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ReservesHelper {
            #[serde(deserialize_with = "deserialize_amounts")]
            amounts: HashMap<Asset, u128>,
        }

        let helper = ReservesHelper::deserialize(deserializer)?;
        Ok(Self {
            amounts: helper.amounts,
        })
    }
}

struct ReservesAmountSerializer<'a> {
    inner: &'a HashMap<Asset, u128>,
}

impl<'a> Serialize for ReservesAmountSerializer<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.inner.len()))?;
        for (asset, amount) in self.inner {
            map.serialize_entry(asset, &amount.to_string())?;
        }
        map.end()
    }
}

fn deserialize_amounts<'de, D>(deserializer: D) -> Result<HashMap<Asset, u128>, D::Error>
where
    D: Deserializer<'de>,
{
    struct AmountMapVisitor;

    impl<'de> Visitor<'de> for AmountMapVisitor {
        type Value = HashMap<Asset, u128>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("Asset 到 u128 的映射，值可为数字或字符串")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut map = HashMap::with_capacity(access.size_hint().unwrap_or(0));
            while let Some((asset, value)) = access.next_entry::<Asset, NumericU128>()? {
                map.insert(asset, value.into_u128()?);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_map(AmountMapVisitor)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumericU128 {
    String(String),
    Number(u128),
}

impl NumericU128 {
    fn into_u128<E>(self) -> Result<u128, E>
    where
        E: de::Error,
    {
        match self {
            NumericU128::String(s) => s
                .parse::<u128>()
                .map_err(|err| E::custom(format!("解析 u128 失败: {}", err))),
            NumericU128::Number(value) => Ok(value),
        }
    }
}

/// 池子状态抽象，定义最小读写能力。
#[async_trait]
pub trait PoolState: Send + Sync {
    /// 返回池子唯一标识。
    fn id(&self) -> &PoolIdentifier;

    /// 获取池子储备信息。
    fn reserves(&self) -> &Reserves;

    /// 应用链上事件对状态进行更新。
    async fn apply_event(&mut self, event: &EventEnvelope) -> Result<(), StateError>;

    /// 生成沙盒快照，供模拟使用。
    fn snapshot(&self) -> PoolSnapshot;
}

/// 池子快照，用于沙盒模拟快速恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSnapshot {
    pub id: PoolIdentifier,
    pub reserves: Reserves,
    pub extra: serde_yaml::Value,
}

/// 状态相关错误。
#[derive(thiserror::Error, Debug)]
pub enum StateError {
    #[error("池子未找到")]
    NotFound,
    #[error("状态更新失败: {0}")]
    UpdateFailed(String),
    #[error("序列化失败: {0}")]
    Serialize(String),
    #[error("数据无效: {0}")]
    Invalid(String),
    #[error("存储错误: {0}")]
    Storage(String),
    #[error("IO 错误: {0}")]
    Io(String),
}
