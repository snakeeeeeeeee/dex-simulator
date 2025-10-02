use anyhow::Result;
use dex_simulator_core::config::AppConfig;
use dex_simulator_core::state::snapshot::{FileSnapshotStore, SnapshotStore};

use super::utils::{parse_pool_identifier, sample_pool_identifier};

/// 打印池子状态，优先读取快照存储。
pub async fn dump_pool_state(config: &AppConfig, pool_id: String) -> Result<()> {
    let identifier = parse_pool_identifier(&pool_id)
        .unwrap_or_else(|| sample_pool_identifier(config.network.chain_id));
    let store = FileSnapshotStore::new(&config.runtime.snapshot_path);
    match store.load(&identifier).await? {
        Some(snapshot) => {
            log::info!("读取快照成功: {:?}", snapshot);
        }
        None => {
            log::warn!("未找到池子状态，输入: {}", pool_id);
        }
    }
    Ok(())
}
