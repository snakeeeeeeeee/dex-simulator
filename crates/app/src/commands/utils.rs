use ethers::types::{Address, H256};
use serde_json::json;
use std::str::FromStr;

use dex_simulator_core::dex::pancake_v2::state::PancakeV2SnapshotExtra;
use dex_simulator_core::event::queue::{EventQueue, InMemoryEventQueue};
use dex_simulator_core::event::store::{EventStore, InMemoryEventStore};
use dex_simulator_core::event::{EventEnvelope, EventKind};
use dex_simulator_core::state::PoolSnapshot;
use dex_simulator_core::types::{Asset, ChainNamespace, PoolIdentifier, PoolType};

/// 解析池子标识。
pub fn parse_pool_identifier(input: &str) -> Option<PoolIdentifier> {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() != 4 {
        return None;
    }
    let chain_id = parts[0].parse().ok()?;
    let address = Address::from_str(parts[2]).ok()?;
    let pool_type = match parts[3] {
        "pancake_v2" => PoolType::PancakeV2,
        "pancake_v3" => PoolType::PancakeV3,
        "pancake_v4" => PoolType::PancakeV4,
        "uniswap_v2" => PoolType::UniswapV2,
        "uniswap_v3" => PoolType::UniswapV3,
        other => PoolType::Other(other.to_string()),
    };
    Some(PoolIdentifier {
        chain_namespace: ChainNamespace::Evm,
        chain_id,
        dex: if parts[1].is_empty() {
            pool_type.label()
        } else {
            parts[1].to_string()
        },
        address,
        pool_type,
    })
}

/// 构造示例池子标识。
pub fn sample_pool_identifier(chain_id: u64) -> PoolIdentifier {
    PoolIdentifier {
        chain_namespace: ChainNamespace::Evm,
        chain_id,
        dex: PoolType::PancakeV2.label(),
        address: Address::from_str("0x000000000000000000000000000000000000abcd")
            .unwrap_or_else(|_| Address::zero()),
        pool_type: PoolType::PancakeV2,
    }
}

/// 构造示例池子元数据。
pub fn sample_pool(chain_id: u64) -> (PoolIdentifier, Asset, Asset) {
    let pool_id = sample_pool_identifier(chain_id);
    let token0 = Asset {
        address: Address::from_str("0x0000000000000000000000000000000000000010")
            .unwrap_or_else(|_| Address::zero()),
        symbol: "WBNB".into(),
        decimals: 18,
    };
    let token1 = Asset {
        address: Address::from_str("0x0000000000000000000000000000000000000020")
            .unwrap_or_else(|_| Address::zero()),
        symbol: "BUSD".into(),
        decimals: 18,
    };
    (pool_id, token0, token1)
}

/// 地址转字符串。
pub fn format_address(addr: Address) -> String {
    format!("{:#x}", addr)
}

/// 格式化池子标识。
pub fn format_identifier(id: &PoolIdentifier) -> String {
    format!(
        "{}:{}:{:#x}:{}",
        id.chain_id,
        id.dex,
        id.address,
        pool_type_label(&id.pool_type)
    )
}

/// 池子类型标签。
pub fn pool_type_label(pool_type: &PoolType) -> &'static str {
    match pool_type {
        PoolType::PancakeV2 => "pancake_v2",
        PoolType::PancakeV3 => "pancake_v3",
        PoolType::PancakeV4 => "pancake_v4",
        PoolType::UniswapV2 => "uniswap_v2",
        PoolType::UniswapV3 => "uniswap_v3",
        PoolType::Other(_) => "other",
    }
}

/// 快照转 JSON。
pub fn snapshot_to_json(snapshot: &PoolSnapshot) -> serde_json::Value {
    let extra = serde_yaml::from_value::<PancakeV2SnapshotExtra>(snapshot.extra.clone()).ok();
    let reserve_list: Vec<_> = snapshot
        .reserves
        .amounts
        .iter()
        .map(|(asset, amount)| {
            json!({
                "token": format_address(asset.address),
                "symbol": asset.symbol,
                "decimals": asset.decimals,
                "amount": amount,
            })
        })
        .collect();
    json!({
        "pool": format_identifier(&snapshot.id),
        "reserves": reserve_list,
        "extra": extra,
    })
}

/// 统计信息（浮点）。
pub fn stats_summary(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return json!(null);
    }
    let min = values.iter().fold(f64::INFINITY, |acc, v| acc.min(*v));
    let max = values.iter().fold(f64::NEG_INFINITY, |acc, v| acc.max(*v));
    let sum: f64 = values.iter().sum();
    let avg = sum / values.len() as f64;
    json!({
        "min": min,
        "max": max,
        "avg": avg,
    })
}

/// 统计信息（u128）。
pub fn stats_summary_u128(values: &[u128]) -> serde_json::Value {
    if values.is_empty() {
        return json!(null);
    }
    let min = values.iter().min().cloned().unwrap_or(0);
    let max = values.iter().max().cloned().unwrap_or(0);
    let sum: u128 = values.iter().sum();
    let avg = sum / (values.len() as u128);
    json!({
        "min": min,
        "max": max,
        "avg": avg,
    })
}

/// 地址白名单解析。
pub fn parse_address_list(list: Option<&Vec<String>>) -> Option<Vec<Address>> {
    let mut result = Vec::new();
    if let Some(items) = list {
        for item in items {
            match Address::from_str(item) {
                Ok(addr) => result.push(addr),
                Err(err) => log::warn!("无效地址 {}: {}", item, err),
            }
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

#[allow(dead_code)]
pub async fn bootstrap_components() {
    let queue = InMemoryEventQueue::shared();
    let store = InMemoryEventStore::new();
    let _ = queue
        .enqueue(EventEnvelope {
            kind: EventKind::Unknown,
            block_number: 0,
            transaction_hash: Default::default(),
            log_index: 0,
            address: Default::default(),
            topics: vec![],
            payload: Default::default(),
        })
        .await;
    let _ = store
        .append(&EventEnvelope {
            kind: EventKind::Unknown,
            block_number: 0,
            transaction_hash: Default::default(),
            log_index: 0,
            address: Default::default(),
            topics: vec![],
            payload: Default::default(),
        })
        .await;
}

#[allow(dead_code)]
pub fn sample_snapshot() -> PoolSnapshot {
    let (pool_id, token0, token1) = sample_pool(56);
    let mut state =
        dex_simulator_core::dex::pancake_v2::PancakeV2PoolState::new(pool_id, token0, token1);
    state.set_reserves(1_000_000_000_000_000_000u128, 500_000_000_000_000_000u128);
    state.to_snapshot().expect("快照生成失败")
}

/// 工具函数：构造日志主题。
pub fn address_to_topic(address: Address) -> H256 {
    let mut topic_bytes = [0u8; 32];
    topic_bytes[12..].copy_from_slice(address.as_bytes());
    H256::from(topic_bytes)
}
