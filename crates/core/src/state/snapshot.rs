use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ethers::abi::AbiEncode;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

use super::{PoolSnapshot, StateError};
use crate::types::PoolIdentifier;

/// 快照存储抽象，负责保存与恢复池子状态。
#[async_trait]
pub trait SnapshotStore: Send + Sync {
    async fn save(&self, snapshot: &PoolSnapshot) -> Result<(), StateError>;
    async fn load(&self, id: &PoolIdentifier) -> Result<Option<PoolSnapshot>, StateError>;
    async fn remove(&self, id: &PoolIdentifier) -> Result<(), StateError>;
    async fn list(&self) -> Result<Vec<PoolSnapshot>, StateError>;
}

/// 内存快照存储，实现轻量级的易失缓存。
#[derive(Debug, Default, Clone)]
pub struct InMemorySnapshotStore {
    inner: Arc<RwLock<HashMap<PoolIdentifier, PoolSnapshot>>>,
}

impl InMemorySnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SnapshotStore for InMemorySnapshotStore {
    async fn save(&self, snapshot: &PoolSnapshot) -> Result<(), StateError> {
        let mut guard = self.inner.write().await;
        guard.insert(snapshot.id.clone(), snapshot.clone());
        Ok(())
    }

    async fn load(&self, id: &PoolIdentifier) -> Result<Option<PoolSnapshot>, StateError> {
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

/// 基于本地文件系统的 YAML 快照存储实现。
#[derive(Debug, Clone)]
pub struct FileSnapshotStore {
    root: PathBuf,
}

impl FileSnapshotStore {
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn snapshot_path(&self, id: &PoolIdentifier) -> PathBuf {
        let address_hex = id.address.encode_hex();
        let pool_type = format!("{:?}", id.pool_type).to_lowercase();
        let file_name = format!(
            "{}_{}_{}_{}.yaml",
            id.chain_id,
            sanitize(&id.dex),
            sanitize(&address_hex),
            sanitize(&pool_type)
        );
        self.root.join(file_name)
    }
}

#[async_trait]
impl SnapshotStore for FileSnapshotStore {
    async fn save(&self, snapshot: &PoolSnapshot) -> Result<(), StateError> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|err| StateError::Io(err.to_string()))?;

        let serialized = match serde_yaml::to_string(snapshot) {
            Ok(data) => data,
            Err(err) => {
                tracing::warn!(
                    "快照序列化失败, pool={:#x}, 错误: {}",
                    snapshot.id.address,
                    err
                );
                return Ok(());
            }
        };
        let path = self.snapshot_path(&snapshot.id);
        let mut file = fs::File::create(&path)
            .await
            .map_err(|err| StateError::Io(err.to_string()))?;
        file.write_all(serialized.as_bytes())
            .await
            .map_err(|err| StateError::Io(err.to_string()))?;
        file.flush()
            .await
            .map_err(|err| StateError::Io(err.to_string()))?;
        Ok(())
    }

    async fn load(&self, id: &PoolIdentifier) -> Result<Option<PoolSnapshot>, StateError> {
        let path = self.snapshot_path(id);
        match fs::read(&path).await {
            Ok(bytes) => {
                let snapshot = serde_yaml::from_slice(&bytes)
                    .map_err(|err| StateError::Serialize(err.to_string()))?;
                Ok(Some(snapshot))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(StateError::Io(err.to_string())),
        }
    }

    async fn remove(&self, id: &PoolIdentifier) -> Result<(), StateError> {
        let path = self.snapshot_path(id);
        match fs::remove_file(&path).await {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(StateError::Io(err.to_string())),
        }
    }

    async fn list(&self) -> Result<Vec<PoolSnapshot>, StateError> {
        let mut snapshots = Vec::new();
        let mut dir = match fs::read_dir(&self.root).await {
            Ok(dir) => dir,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(snapshots),
            Err(err) => return Err(StateError::Io(err.to_string())),
        };

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|err| StateError::Io(err.to_string()))?
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            match fs::read(&path).await {
                Ok(bytes) => {
                    if let Ok(snapshot) = serde_yaml::from_slice::<PoolSnapshot>(&bytes) {
                        snapshots.push(snapshot);
                    }
                }
                Err(err) => {
                    tracing::warn!("读取快照文件失败: {:?}, 错误: {}", path, err);
                }
            }
        }

        Ok(snapshots)
    }
}

fn sanitize(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
