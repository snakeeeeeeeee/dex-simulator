use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ethers::abi::{encode, Token};
use ethers::types::{Address, Bytes, H256, U256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use dex_simulator_core::config::AppConfig;
use dex_simulator_core::dex::pancake_v2::{
    burn_topic, event_topics, mint_topic, pair_created_topic, swap_topic, sync_topic,
    PancakeV2Config, PancakeV2EventHandler,
};
use dex_simulator_core::dex::pancake_v3::{
    event as pancake_v3_event,
    handler::{PancakeV3Config, PancakeV3EventHandler},
};
use dex_simulator_core::dex::router::DexEventRouter;
use dex_simulator_core::event::queue::{EventQueue, InMemoryEventQueue};
use dex_simulator_core::event::store::{EventStore, InMemoryEventStore};
use dex_simulator_core::event::{
    source::{EthersEventSource, EthersEventSourceConfig},
    EventEnvelope, EventKind, EventListener, EventListenerError, EventSink, EventSource,
    LocalEventListener, MockEventSource,
};
use dex_simulator_core::state::repository::{InMemoryPoolRepository, PoolRepository};
use dex_simulator_core::state::snapshot::{FileSnapshotStore, SnapshotStore};

use super::utils::{address_to_topic, parse_address_list};

/// 启动监听流程。
pub async fn listen(config: &AppConfig, sample_events: u64) -> Result<()> {
    let queue: Arc<dyn EventQueue> = InMemoryEventQueue::shared();
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let listener = Arc::new(LocalEventListener::new(queue.clone(), store.clone()));

    let repo_concrete = Arc::new(InMemoryPoolRepository::new());
    let repository: Arc<dyn PoolRepository> = repo_concrete.clone();
    let snapshot_store_concrete = Arc::new(FileSnapshotStore::new(&config.runtime.snapshot_path));
    let snapshot_store: Arc<dyn SnapshotStore> = snapshot_store_concrete.clone();
    let token_whitelist = parse_address_list(config.tokens.whitelist.as_ref());
    let handler_v2 = Arc::new(PancakeV2EventHandler::new(
        PancakeV2Config {
            chain_id: config.network.chain_id,
            dex: "pancakeswap".into(),
            factory_address: Address::from_str("0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73")
                .unwrap_or_else(|_| Address::zero()),
            default_decimals: 18,
            token_whitelist: token_whitelist.clone(),
        },
        repository.clone(),
        Some(snapshot_store.clone()),
    ));

    let mut router = DexEventRouter::new();
    router.register(handler_v2.clone());

    let mut handler_v3_opt: Option<Arc<PancakeV3EventHandler>> = None;
    if let Some(path) = config.dex.pancake_v3_bootstrap.as_ref() {
        if Path::new(path).exists() {
            match PancakeV3EventHandler::from_config_file(
                path,
                PancakeV3Config {
                    chain_id: config.network.chain_id,
                    dex: "pancakeswap".into(),
                    default_decimals: 18,
                },
                repository.clone(),
                Some(snapshot_store.clone()),
            )
            .await
            {
                Ok(handler_v3) => {
                    let handler_arc = Arc::new(handler_v3);
                    router.register(handler_arc.clone());
                    handler_v3_opt = Some(handler_arc);
                }
                Err(err) => log::warn!("加载 V3 bootstrap 失败: {}", err),
            }
        } else {
            log::warn!("未找到 V3 bootstrap 配置文件: {}", path);
        }
    }

    let router = Arc::new(router);
    listener.subscribe(Arc::new(RouterSink {
            router: router.clone(),
        }) as Arc<dyn EventSink>)
        .await;
    let logging_sink: Arc<dyn EventSink> = Arc::new(LoggingEventSink::default());
    listener.subscribe(logging_sink).await;

    let source: Arc<dyn EventSource> = if sample_events == 0 {
        build_live_event_source(
            config,
            &handler_v2,
            handler_v3_opt.as_ref(),
            repository.clone(),
        )
        .await
        .map_err(|err| anyhow!(err.to_string()))?
    } else {
        let events = build_mock_events(sample_events)?;
        Arc::new(MockEventSource::new(events))
    };
    listener.attach_source(source).await;

    listener.clone().start().await?;
    if sample_events == 0 {
        log::info!("实时监听已启动，按 Ctrl+C 结束...");
        tokio::signal::ctrl_c()
            .await
            .map_err(|err| anyhow!(format!("等待 Ctrl+C 失败: {}", err)))?;
    } else {
        let wait_ms = 300 * (sample_events + 4);
        sleep(Duration::from_millis(wait_ms)).await;
    }
    listener.clone().stop().await?;

    let snapshots = repo_concrete.list().await?;
    for snapshot in snapshots {
        log::info!("监听结束，池子状态: {:?}", snapshot);
    }
    Ok(())
}

async fn build_live_event_source(
    config: &AppConfig,
    handler_v2: &Arc<PancakeV2EventHandler>,
    handler_v3: Option<&Arc<PancakeV3EventHandler>>,
    repository: Arc<dyn PoolRepository>,
) -> Result<Arc<dyn EventSource>, EventListenerError> {
    let mut topic_kinds: HashMap<H256, EventKind> = HashMap::new();
    topic_kinds.insert(pair_created_topic(), EventKind::PairCreated);
    topic_kinds.insert(mint_topic(), EventKind::Mint);
    topic_kinds.insert(burn_topic(), EventKind::Burn);
    topic_kinds.insert(swap_topic(), EventKind::Swap);
    topic_kinds.insert(sync_topic(), EventKind::Sync);
    topic_kinds.insert(pancake_v3_event::mint_topic(), EventKind::Mint);
    topic_kinds.insert(pancake_v3_event::burn_topic(), EventKind::Burn);
    topic_kinds.insert(pancake_v3_event::swap_topic(), EventKind::Swap);
    topic_kinds.insert(pancake_v3_event::initialize_topic(), EventKind::Sync);

    let mut topic_set: HashSet<H256> = HashSet::new();
    for topic in event_topics() {
        topic_set.insert(*topic);
    }
    for topic in pancake_v3_event::event_topics() {
        topic_set.insert(*topic);
    }

    let mut address_set: HashSet<Address> = HashSet::new();
    if let Some(factory) = handler_v2.factory_address() {
        address_set.insert(factory);
    }

    let snapshots = repository
        .list()
        .await
        .map_err(|err| EventListenerError::Internal(err.to_string()))?;
    for snapshot in snapshots {
        address_set.insert(snapshot.id.address);
    }

    if let Some(handler_v3) = handler_v3 {
        for addr in handler_v3.tracked_pools() {
            address_set.insert(addr);
        }
    }

    let mut topics: Vec<H256> = topic_set.into_iter().collect();
    topics.sort_unstable();

    let mut addresses: Vec<Address> = address_set.into_iter().collect();
    addresses.sort_unstable();

    let source_config = EthersEventSourceConfig {
        ws_endpoint: config.network.ws_endpoint.clone(),
        http_endpoint: config.network.http_endpoint.clone(),
        addresses,
        topics,
        topic_kinds,
        backfill_blocks: config.network.event_backfill_blocks,
        chunk_size: config.network.backfill_chunk_size.max(1),
        retry_interval: Duration::from_secs(config.network.ws_retry_secs.max(1)),
        channel_capacity: 2_048,
        warn_delay_ms: config.monitoring.event_delay_warn_ms,
        stats_event_interval: config.monitoring.event_stats_interval,
    };

    let live_source = EthersEventSource::connect(source_config).await?;
    let live_source: Arc<dyn EventSource> = live_source;
    Ok(live_source)
}

fn build_mock_events(swap_events: u64) -> Result<Vec<EventEnvelope>, anyhow::Error> {
    let factory = Address::from_str("0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73")?;
    let pair = Address::from_str("0x000000000000000000000000000000000000abcd")?;
    let token0 = Address::from_str("0x0000000000000000000000000000000000000010")?;
    let token1 = Address::from_str("0x0000000000000000000000000000000000000020")?;
    let sender = Address::from_str("0x0000000000000000000000000000000000001234")?;
    let to = Address::from_str("0x0000000000000000000000000000000000005678")?;

    let mut events = Vec::new();
    events.push(EventEnvelope {
        kind: EventKind::PairCreated,
        block_number: 1,
        transaction_hash: H256::from_low_u64_be(1),
        log_index: 0,
        address: factory,
        topics: vec![
            pair_created_topic(),
            address_to_topic(token0),
            address_to_topic(token1),
        ],
        payload: Bytes::from(encode(&[
            Token::Address(pair),
            Token::Uint(U256::from(1u64)),
        ])),
    });

    events.push(EventEnvelope {
        kind: EventKind::Mint,
        block_number: 2,
        transaction_hash: H256::from_low_u64_be(2),
        log_index: 0,
        address: pair,
        topics: vec![mint_topic(), address_to_topic(sender)],
        payload: Bytes::from(encode(&[
            Token::Uint(U256::from(1_000_000_000_000_000_000u128)),
            Token::Uint(U256::from(500_000_000_000_000_000u128)),
        ])),
    });

    events.push(EventEnvelope {
        kind: EventKind::Sync,
        block_number: 2,
        transaction_hash: H256::from_low_u64_be(2),
        log_index: 1,
        address: pair,
        topics: vec![sync_topic()],
        payload: Bytes::from(encode(&[
            Token::Uint(U256::from(1_000_000_000_000_000_000u128)),
            Token::Uint(U256::from(500_000_000_000_000_000u128)),
        ])),
    });

    for i in 0..swap_events {
        events.push(EventEnvelope {
            kind: EventKind::Swap,
            block_number: 3 + i,
            transaction_hash: H256::from_low_u64_be(10 + i),
            log_index: 0,
            address: pair,
            topics: vec![swap_topic(), address_to_topic(sender), address_to_topic(to)],
            payload: Bytes::from(encode(&[
                Token::Uint(U256::from(100_000_000_000_000_000u128)),
                Token::Uint(U256::zero()),
                Token::Uint(U256::zero()),
                Token::Uint(U256::from(45_000_000_000_000_000u128)),
            ])),
        });

        events.push(EventEnvelope {
            kind: EventKind::Sync,
            block_number: 3 + i,
            transaction_hash: H256::from_low_u64_be(10 + i),
            log_index: 1,
            address: pair,
            topics: vec![sync_topic()],
            payload: Bytes::from(encode(&[
                Token::Uint(U256::from(1_100_000_000_000_000_000u128)),
                Token::Uint(U256::from(455_000_000_000_000_000u128)),
            ])),
        });
    }

    Ok(events)
}

#[derive(Default)]
struct LoggingEventSink;

#[async_trait]
impl EventSink for LoggingEventSink {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        log::info!(
            "接收到事件: kind={:?}, block={}, tx={:#x}, log_index={}",
            event.kind,
            event.block_number,
            event.transaction_hash,
            event.log_index
        );
        Ok(())
    }
}

struct RouterSink {
    router: Arc<DexEventRouter>,
}

#[async_trait]
impl EventSink for RouterSink {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        self.router.dispatch(event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dex_simulator_core::config::load_config;
    use std::path::PathBuf;
    use tokio::fs;

    #[tokio::test]
    async fn test_listen_with_mock_events() {
        let config_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/default.yaml");
        let mut config = load_config(&config_path).expect("加载配置失败");
        let unique_path = format!(
            "target/test-snapshots-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        config.runtime.snapshot_path = unique_path.clone();

        listen(&config, 1).await.expect("监听逻辑执行失败");

        let mut dir = fs::read_dir(&config.runtime.snapshot_path)
            .await
            .expect("读取快照目录失败");
        let mut has_file = false;
        while let Some(entry) = dir.next_entry().await.expect("读取目录项失败") {
            if entry.file_type().await.expect("获取文件类型失败").is_file() {
                has_file = true;
                break;
            }
        }
        assert!(has_file, "应当生成至少一个快照文件");

        let _ = fs::remove_dir_all(&config.runtime.snapshot_path).await;
    }
}
