use std::sync::Arc;

use async_trait::async_trait;

use super::{EventEnvelope, EventStore, EventStoreError};

/// RocksDB 实现占位。若需启用，请在 Cargo feature 中开启并补充具体逻辑。
#[derive(Debug, Clone)]
pub struct RocksDbEventStore {
    _inner: Arc<()>,
}

impl RocksDbEventStore {
    pub fn new(_path: impl AsRef<std::path::Path>) -> Self {
        Self {
            _inner: Arc::new(()),
        }
    }
}

#[async_trait]
impl EventStore for RocksDbEventStore {
    async fn append(&self, _event: &EventEnvelope) -> Result<(), EventStoreError> {
        Err(EventStoreError::Storage(
            "RocksDB 存储尚未启用，请在后续阶段实现".to_string(),
        ))
    }

    async fn load_range(
        &self,
        _from_block: u64,
        _to_block: Option<u64>,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        Err(EventStoreError::Storage(
            "RocksDB 存储尚未启用，请在后续阶段实现".to_string(),
        ))
    }

    async fn last_block(&self) -> Result<Option<u64>, EventStoreError> {
        Ok(None)
    }
}
