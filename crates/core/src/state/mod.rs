pub mod onchain;
pub mod repository;
pub mod snapshot;

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::event::EventEnvelope;
use crate::types::{Asset, PoolIdentifier};

/// 通用储备表示。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
