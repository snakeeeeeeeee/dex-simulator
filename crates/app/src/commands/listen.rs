use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ethers::abi::{encode, Token};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, BlockNumber, Bytes, Filter, ValueOrArray, H256, U256};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use dex_simulator_core::config::AppConfig;
use dex_simulator_core::dex::bootstrap::{
    load_v2_bootstrap, load_v3_bootstrap, V2BootstrapPool, V3BootstrapPool,
};
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
use dex_simulator_core::state::onchain::OnChainStateFetcher;
use dex_simulator_core::state::repository::{InMemoryPoolRepository, PoolRepository};
use dex_simulator_core::state::snapshot::{InMemorySnapshotStore, SnapshotStore};
use dex_simulator_core::types::{Asset, PoolType};

use super::utils::{address_to_topic, parse_address_list, pool_type_label};

/// 启动监听流程。
pub async fn listen(config: &AppConfig, sample_events: u64) -> Result<()> {
    let queue: Arc<dyn EventQueue> = InMemoryEventQueue::shared();
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let listener = Arc::new(LocalEventListener::new(queue.clone(), store.clone()));

    let repo_concrete = Arc::new(InMemoryPoolRepository::new());
    let repository: Arc<dyn PoolRepository> = repo_concrete.clone();
    let snapshot_store_concrete = Arc::new(InMemorySnapshotStore::new());
    let snapshot_store: Arc<dyn SnapshotStore> = snapshot_store_concrete.clone();
    let token_whitelist = parse_address_list(config.tokens.whitelist.as_ref());

    let v2_bootstrap = if let Some(path) = config.dex.pancake_v2_bootstrap.as_ref() {
        load_v2_bootstrap(path).map_err(|err| anyhow!(err.to_string()))?
    } else {
        Vec::new()
    };

    let v3_bootstrap = if let Some(path) = config.dex.pancake_v3_bootstrap.as_ref() {
        load_v3_bootstrap(path).map_err(|err| anyhow!(err.to_string()))?
    } else {
        Vec::new()
    };

    let v2_token_map: Option<HashMap<Address, (Asset, Asset)>> = if v2_bootstrap.is_empty() {
        None
    } else {
        Some(
            v2_bootstrap
                .iter()
                .map(|entry| (entry.pool, (entry.token0.clone(), entry.token1.clone())))
                .collect(),
        )
    };

    let provider = match Provider::<Http>::try_from(config.network.http_endpoint.as_str()) {
        Ok(p) => Some(Arc::new(p)),
        Err(err) => {
            log::warn!("创建 HTTP Provider 失败，跳过启动快照与历史回放: {}", err);
            None
        }
    };

    let fetcher = provider
        .as_ref()
        .map(|prov| OnChainStateFetcher::new(prov.clone(), config.network.chain_id));

    let anchor_block = if sample_events == 0 {
        if let Some(prov) = provider.as_ref() {
            match prov.get_block_number().await {
                Ok(number) => Some(number.as_u64()),
                Err(err) => {
                    log::warn!("获取最新块失败，跳过历史回放: {}", err);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    if sample_events == 0 {
        if let Some(fetcher) = fetcher.as_ref() {
            for entry in &v2_bootstrap {
                match fetcher.fetch_v2_snapshot(entry).await {
                    Ok(snapshot) => {
                        repo_concrete
                            .upsert(snapshot.clone())
                            .await
                            .map_err(|err| anyhow!(err.to_string()))?;
                        if let Err(err) = snapshot_store.save(&snapshot).await {
                            log::warn!("写入 V2 快照失败: pool={:#x}, 错误: {}", entry.pool, err);
                        }
                    }
                    Err(err) => {
                        log::warn!(
                            "V2 池子链上快照拉取失败: pool={:#x}, 错误: {}",
                            entry.pool,
                            err
                        );
                    }
                }
            }

            for entry in &v3_bootstrap {
                match fetcher.fetch_v3_snapshot(entry).await {
                    Ok(snapshot) => {
                        repo_concrete
                            .upsert(snapshot.clone())
                            .await
                            .map_err(|err| anyhow!(err.to_string()))?;
                        if let Err(err) = snapshot_store.save(&snapshot).await {
                            log::warn!("写入 V3 快照失败: pool={:#x}, 错误: {}", entry.pool, err);
                        }
                    }
                    Err(err) => {
                        log::warn!(
                            "V3 池子链上快照拉取失败: pool={:#x}, 错误: {}",
                            entry.pool,
                            err
                        );
                    }
                }
            }
        }
    }

    let handler_v2 = Arc::new(PancakeV2EventHandler::new(
        PancakeV2Config {
            chain_id: config.network.chain_id,
            dex: PoolType::PancakeV2.label(),
            factory_address: Address::from_str("0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73")
                .unwrap_or_else(|_| Address::zero()),
            default_decimals: 18,
            token_whitelist: token_whitelist.clone(),
        },
        repository.clone(),
        Some(snapshot_store.clone()),
        v2_token_map.clone(),
    ));

    let mut router_inner = DexEventRouter::new();
    router_inner.register(handler_v2.clone());

    if !v3_bootstrap.is_empty() {
        let handler_v3 = PancakeV3EventHandler::new(
            PancakeV3Config {
                chain_id: config.network.chain_id,
                dex: PoolType::PancakeV3.label(),
                default_decimals: 18,
            },
            repository.clone(),
            Some(snapshot_store.clone()),
            v3_bootstrap.clone(),
        );
        let handler_arc = Arc::new(handler_v3);
        router_inner.register(handler_arc);
    }

    let router = Arc::new(router_inner);
    listener
        .subscribe(Arc::new(RouterSink {
            router: router.clone(),
        }) as Arc<dyn EventSink>)
        .await;

    let metadata =
        build_subscription_metadata(repository.clone(), &v2_bootstrap, &v3_bootstrap).await?;

    let logging_sink: Arc<dyn EventSink> =
        Arc::new(LoggingEventSink::new(metadata.pool_labels.clone()));
    listener.subscribe(logging_sink).await;

    if sample_events == 0 {
        if let (Some(prov), Some(anchor)) = (provider.as_ref(), anchor_block) {
            match prov.get_block_number().await {
                Ok(number) => {
                    let latest_after = number.as_u64();
                    if latest_after > anchor {
                        replay_events(
                            prov.clone(),
                            router.clone(),
                            &metadata,
                            anchor + 1,
                            latest_after,
                            config.network.backfill_chunk_size.max(1),
                        )
                        .await?;
                    }
                }
                Err(err) => {
                    log::warn!("回放前获取最新块失败: {}", err);
                }
            }
        }
    }

    let source: Arc<dyn EventSource> = if sample_events == 0 {
        build_live_event_source(config, &metadata)
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

struct SubscriptionMetadata {
    addresses: Vec<Address>,
    topics: Vec<H256>,
    topic_kinds: HashMap<H256, EventKind>,
    pool_labels: Arc<HashMap<Address, String>>,
}

async fn build_subscription_metadata(
    repository: Arc<dyn PoolRepository>,
    v2_bootstrap: &[V2BootstrapPool],
    v3_bootstrap: &[V3BootstrapPool],
) -> Result<SubscriptionMetadata> {
    let mut topic_kinds: HashMap<H256, EventKind> = HashMap::new();
    for topic in event_topics() {
        topic_kinds.entry(*topic).or_insert(EventKind::Unknown);
    }
    topic_kinds.insert(pair_created_topic(), EventKind::PairCreated);
    topic_kinds.insert(mint_topic(), EventKind::Mint);
    topic_kinds.insert(burn_topic(), EventKind::Burn);
    topic_kinds.insert(swap_topic(), EventKind::Swap);
    topic_kinds.insert(sync_topic(), EventKind::Sync);

    for topic in pancake_v3_event::event_topics() {
        topic_kinds.entry(*topic).or_insert(EventKind::Unknown);
    }
    topic_kinds.insert(pancake_v3_event::mint_topic(), EventKind::Mint);
    topic_kinds.insert(pancake_v3_event::burn_topic(), EventKind::Burn);
    topic_kinds.insert(pancake_v3_event::swap_topic(), EventKind::Swap);
    topic_kinds.insert(pancake_v3_event::initialize_topic(), EventKind::Sync);
    topic_kinds.insert(pancake_v3_event::collect_topic(), EventKind::Collect);

    let mut address_set: HashSet<Address> = HashSet::new();
    let mut labels: HashMap<Address, String> = HashMap::new();
    for pool in v2_bootstrap {
        address_set.insert(pool.pool);
        labels.insert(pool.pool, PoolType::PancakeV2.label());
    }
    for pool in v3_bootstrap {
        address_set.insert(pool.pool);
        labels.insert(pool.pool, PoolType::PancakeV3.label());
    }

    if let Ok(snapshots) = repository.list().await {
        for snapshot in snapshots {
            address_set.insert(snapshot.id.address);
            labels
                .entry(snapshot.id.address)
                .or_insert_with(|| pool_type_label(&snapshot.id.pool_type).to_string());
        }
    }

    let mut topics: Vec<H256> = topic_kinds.keys().copied().collect();
    topics.sort_unstable();

    let mut addresses: Vec<Address> = address_set.into_iter().collect();
    addresses.sort_unstable();

    Ok(SubscriptionMetadata {
        addresses,
        topics,
        topic_kinds,
        pool_labels: Arc::new(labels),
    })
}

async fn build_live_event_source(
    config: &AppConfig,
    metadata: &SubscriptionMetadata,
) -> Result<Arc<dyn EventSource>, EventListenerError> {
    let source_config = EthersEventSourceConfig {
        ws_endpoint: config.network.ws_endpoint.clone(),
        http_endpoint: config.network.http_endpoint.clone(),
        addresses: metadata.addresses.clone(),
        topics: metadata.topics.clone(),
        topic_kinds: metadata.topic_kinds.clone(),
        backfill_blocks: config.network.event_backfill_blocks,
        chunk_size: config.network.backfill_chunk_size.max(1),
        retry_interval: Duration::from_secs(config.network.ws_retry_secs.max(1)),
        channel_capacity: 2_048,
        warn_delay_ms: config.monitoring.event_delay_warn_ms,
        stats_event_interval: config.monitoring.event_stats_interval,
    };
    let source = EthersEventSource::connect(source_config).await?;
    Ok(source as Arc<dyn EventSource>)
}

async fn replay_events(
    provider: Arc<Provider<Http>>,
    router: Arc<DexEventRouter>,
    metadata: &SubscriptionMetadata,
    from_block: u64,
    to_block: u64,
    chunk_size: u64,
) -> Result<()> {
    if metadata.addresses.is_empty() || metadata.topics.is_empty() {
        return Ok(());
    }

    let mut current = from_block;
    let effective_chunk = chunk_size.max(1);

    while current <= to_block {
        let mut chunk = effective_chunk;
        let mut finished_chunk = false;
        while !finished_chunk {
            let end = current
                .saturating_add(chunk.saturating_sub(1))
                .min(to_block);
            let mut filter = Filter::new();
            filter = filter.address(ValueOrArray::Array(metadata.addresses.clone()));
            filter = filter.topic0(ValueOrArray::Array(metadata.topics.clone()));
            filter = filter
                .from_block(BlockNumber::Number(current.into()))
                .to_block(BlockNumber::Number(end.into()));

            match provider.get_logs(&filter).await {
                Ok(logs) => {
                    for log in logs {
                        if log.removed.unwrap_or(false) {
                            continue;
                        }
                        let kind = log
                            .topics
                            .get(0)
                            .and_then(|topic| metadata.topic_kinds.get(topic))
                            .cloned()
                            .unwrap_or(EventKind::Unknown);
                        let event = EventEnvelope {
                            kind,
                            block_number: log.block_number.unwrap_or_default().as_u64(),
                            transaction_hash: log.transaction_hash.unwrap_or_default(),
                            log_index: log.log_index.map(|idx| idx.as_u64()).unwrap_or(0),
                            address: log.address,
                            topics: log.topics.clone(),
                            payload: Bytes::from(log.data.0.clone()),
                        };

                        if let Err(err) = router.dispatch(event).await {
                            log::error!("历史回放事件失败: {}", err);
                        }
                    }
                    current = end.saturating_add(1);
                    finished_chunk = true;
                }
                Err(err) => {
                    if chunk > 1 {
                        chunk = chunk.saturating_div(2).max(1);
                        log::warn!(
                            "回放日志失败: {}，缩小 chunk 到 {}，范围 [{} - {}]",
                            err,
                            chunk,
                            current,
                            end
                        );
                    } else {
                        log::error!(
                            "回放日志失败且无法再缩小 chunk: 错误={}, 范围 [{} - {}]",
                            err,
                            current,
                            end
                        );
                        current = end.saturating_add(1);
                        finished_chunk = true;
                    }
                }
            }
        }
    }

    Ok(())
}

struct LoggingEventSink {
    labels: Arc<HashMap<Address, String>>,
}

impl LoggingEventSink {
    fn new(labels: Arc<HashMap<Address, String>>) -> Self {
        Self { labels }
    }
}

#[async_trait]
impl EventSink for LoggingEventSink {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        let label = self
            .labels
            .get(&event.address)
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        log::info!(
            "接收到事件: dex={}, kind={:?}, block={}, tx={:#x}, log_index={}",
            label,
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

    #[tokio::test]
    async fn test_listen_with_mock_events() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.join("../..");
        let config_path = project_root.join("config/default.yaml");
        let mut config = load_config(&config_path).expect("加载配置失败");
        if let Some(path) = config.dex.pancake_v2_bootstrap.as_ref() {
            config.dex.pancake_v2_bootstrap =
                Some(project_root.join(path).to_string_lossy().into_owned());
        }
        if let Some(path) = config.dex.pancake_v3_bootstrap.as_ref() {
            config.dex.pancake_v3_bootstrap =
                Some(project_root.join(path).to_string_lossy().into_owned());
        }
        let unique_path = format!(
            "target/test-snapshots-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        config.runtime.snapshot_path = unique_path.clone();

        listen(&config, 1).await.expect("监听逻辑执行失败");
    }
}
