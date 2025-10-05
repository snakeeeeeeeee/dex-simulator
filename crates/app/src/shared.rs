use std::sync::Arc;

use dex_simulator_core::state::repository::{InMemoryPoolRepository, PoolRepository};
use dex_simulator_core::state::snapshot::{InMemorySnapshotStore, SnapshotStore};

/// 运行时共享对象，供监听器、HTTP 服务等组件复用。
#[derive(Clone)]
pub struct SharedObjects {
    pub repository: Arc<dyn PoolRepository>,
    pub snapshot_store: Arc<dyn SnapshotStore>,
}

impl SharedObjects {
    /// 构建使用内存实现的共享状态。
    pub fn in_memory() -> Self {
        let repository: Arc<dyn PoolRepository> = Arc::new(InMemoryPoolRepository::new());
        let snapshot_store: Arc<dyn SnapshotStore> = Arc::new(InMemorySnapshotStore::new());
        Self {
            repository,
            snapshot_store,
        }
    }
}
