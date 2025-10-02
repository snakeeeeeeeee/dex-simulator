use std::sync::Arc;

pub mod rocksdb;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::EventEnvelope;

/// 事件存储接口，负责持久化事件以便回放。
#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, event: &EventEnvelope) -> Result<(), EventStoreError>;
    async fn load_range(
        &self,
        from_block: u64,
        to_block: Option<u64>,
    ) -> Result<Vec<EventEnvelope>, EventStoreError>;
    async fn last_block(&self) -> Result<Option<u64>, EventStoreError>;
}

/// 存储错误定义。
#[derive(thiserror::Error, Debug)]
pub enum EventStoreError {
    #[error("存储失败: {0}")]
    Storage(String),
}

/// 简易内存事件存储，用于阶段构建与单测。
#[derive(Debug, Default, Clone)]
pub struct InMemoryEventStore {
    inner: Arc<RwLock<Vec<EventEnvelope>>>,
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    async fn append(&self, event: &EventEnvelope) -> Result<(), EventStoreError> {
        let mut guard = self.inner.write().await;
        guard.push(event.clone());
        Ok(())
    }

    async fn load_range(
        &self,
        from_block: u64,
        to_block: Option<u64>,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let guard = self.inner.read().await;
        let iter = guard.iter().filter(|e| {
            e.block_number >= from_block && to_block.map(|to| e.block_number <= to).unwrap_or(true)
        });
        Ok(iter.cloned().collect())
    }

    async fn last_block(&self) -> Result<Option<u64>, EventStoreError> {
        let guard = self.inner.read().await;
        Ok(guard.last().map(|e| e.block_number))
    }
}
