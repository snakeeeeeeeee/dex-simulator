use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::event::store::{EventStore, EventStoreError};
use crate::state::repository::PoolRepository;
use crate::state::snapshot::SnapshotStore;
use crate::state::{PoolSnapshot, StateError};
use crate::types::PoolIdentifier;

/// 同步状态相关指标。
#[derive(Debug, Clone)]
pub struct SyncMetrics {
    pub last_synced_block: u64,
    pub drift_bps: f64,
}

/// 状态同步器接口，支持增量与校验操作。
#[async_trait]
pub trait StateSyncer: Send + Sync {
    /// 执行增量同步，将链上事件应用到本地状态。
    async fn incremental_sync(&self) -> Result<SyncMetrics, SyncError>;

    /// 对指定池子进行全量校验与重放。
    async fn full_resync(&self, pool: &PoolIdentifier) -> Result<(), SyncError>;
}

/// 占位同步器，实现空操作以便阶段联调。
#[derive(Default, Debug, Clone)]
pub struct NullStateSyncer;

#[async_trait]
impl StateSyncer for NullStateSyncer {
    async fn incremental_sync(&self) -> Result<SyncMetrics, SyncError> {
        Ok(SyncMetrics {
            last_synced_block: 0,
            drift_bps: 0.0,
        })
    }

    async fn full_resync(&self, _pool: &PoolIdentifier) -> Result<(), SyncError> {
        Ok(())
    }
}

/// 基于事件存储与快照的基础同步器实现。
pub struct BasicStateSyncer {
    event_store: Arc<dyn EventStore>,
    pool_repository: Arc<dyn PoolRepository>,
    snapshot_store: Option<Arc<dyn SnapshotStore>>,
    last_synced_block: AtomicU64,
}

impl BasicStateSyncer {
    pub fn new(
        event_store: Arc<dyn EventStore>,
        pool_repository: Arc<dyn PoolRepository>,
        snapshot_store: Option<Arc<dyn SnapshotStore>>,
    ) -> Self {
        Self {
            event_store,
            pool_repository,
            snapshot_store,
            last_synced_block: AtomicU64::new(0),
        }
    }

    async fn load_snapshots(&self) -> Result<Vec<PoolSnapshot>, SyncError> {
        if let Some(store) = &self.snapshot_store {
            store.list().await.map_err(SyncError::from)
        } else {
            Ok(Vec::new())
        }
    }
}

#[async_trait]
impl StateSyncer for BasicStateSyncer {
    async fn incremental_sync(&self) -> Result<SyncMetrics, SyncError> {
        let last_block = self.event_store.last_block().await?;
        if let Some(block) = last_block {
            self.last_synced_block.store(block, Ordering::SeqCst);
        }

        Ok(SyncMetrics {
            last_synced_block: self.last_synced_block.load(Ordering::SeqCst),
            drift_bps: 0.0,
        })
    }

    async fn full_resync(&self, _pool: &PoolIdentifier) -> Result<(), SyncError> {
        let snapshots = self.load_snapshots().await?;
        for snapshot in snapshots {
            self.pool_repository.upsert(snapshot).await?;
        }
        Ok(())
    }
}

/// 状态同步相关错误。
#[derive(thiserror::Error, Debug)]
pub enum SyncError {
    #[error("RPC 交互失败: {0}")]
    Rpc(String),
    #[error("持久化失败: {0}")]
    Storage(String),
    #[error("未知错误: {0}")]
    Unknown(String),
}

impl From<EventStoreError> for SyncError {
    fn from(err: EventStoreError) -> Self {
        SyncError::Storage(err.to_string())
    }
}

impl From<StateError> for SyncError {
    fn from(err: StateError) -> Self {
        match err {
            StateError::Storage(msg) | StateError::Io(msg) => SyncError::Storage(msg),
            StateError::Invalid(msg) => SyncError::Unknown(msg),
            other => SyncError::Unknown(other.to_string()),
        }
    }
}
