use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{PoolSnapshot, StateError};
use crate::types::PoolIdentifier;

/// 池子状态仓库接口。
#[async_trait]
pub trait PoolRepository: Send + Sync {
    async fn upsert(&self, snapshot: PoolSnapshot) -> Result<(), StateError>;
    async fn get(&self, id: &PoolIdentifier) -> Result<Option<PoolSnapshot>, StateError>;
    async fn remove(&self, id: &PoolIdentifier) -> Result<(), StateError>;
    async fn list(&self) -> Result<Vec<PoolSnapshot>, StateError>;
}

/// 内存实现，用于阶段构建。
#[derive(Debug, Default, Clone)]
pub struct InMemoryPoolRepository {
    inner: Arc<RwLock<HashMap<PoolIdentifier, PoolSnapshot>>>,
}

impl InMemoryPoolRepository {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PoolRepository for InMemoryPoolRepository {
    async fn upsert(&self, snapshot: PoolSnapshot) -> Result<(), StateError> {
        let mut guard = self.inner.write().await;
        guard.insert(snapshot.id.clone(), snapshot);
        Ok(())
    }

    async fn get(&self, id: &PoolIdentifier) -> Result<Option<PoolSnapshot>, StateError> {
        let guard = self.inner.read().await;
        Ok(guard.get(id).cloned())
    }

    async fn remove(&self, id: &PoolIdentifier) -> Result<(), StateError> {
        let mut guard = self.inner.write().await;
        guard.remove(id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PoolSnapshot>, StateError> {
        let guard = self.inner.read().await;
        Ok(guard.values().cloned().collect())
    }
}
