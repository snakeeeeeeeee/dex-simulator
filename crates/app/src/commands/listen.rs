use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ethers::abi::{encode, Token};
use ethers::providers::{Http, Provider};
use ethers::types::{Address, Bytes, H256, U256};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::watch;
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
use dex_simulator_core::types::{Asset, ChainNamespace, PoolIdentifier, PoolType};

use super::utils::{address_to_topic, parse_address_list, pool_type_label};
use crate::shared::SharedObjects;

/// 启动监听流程。
pub async fn listen(config: &AppConfig, sample_events: u64) -> Result<()> {
    listen_with_state(config, sample_events, None, None).await
}

/// 启动监听流程（可复用外部状态及关闭信号）。
pub async fn listen_with_state(
    config: &AppConfig,
    sample_events: u64,
    shared: Option<SharedObjects>,
    shutdown: Option<watch::Receiver<bool>>,
) -> Result<()> {
    let queue: Arc<dyn EventQueue> = InMemoryEventQueue::shared();
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let listener = Arc::new(LocalEventListener::new(queue.clone(), store.clone()));

    let (repository, snapshot_store): (Arc<dyn PoolRepository>, Arc<dyn SnapshotStore>) =
        if let Some(shared_state) = shared {
            (
                shared_state.repository.clone(),
                shared_state.snapshot_store.clone(),
            )
        } else {
            (
                Arc::new(InMemoryPoolRepository::new()),
                Arc::new(InMemorySnapshotStore::new()),
            )
        };
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
    let v3_pool_map: HashMap<Address, V3BootstrapPool> = v3_bootstrap
        .iter()
        .map(|entry| (entry.pool, entry.clone()))
        .collect();

    let provider = match Provider::<Http>::try_from(config.network.http_endpoint.as_str()) {
        Ok(p) => Some(Arc::new(p)),
        Err(err) => {
            tracing::warn!("创建 HTTP Provider 失败，后续无法按需补齐状态: {}", err);
            None
        }
    };
    let fetcher = provider.as_ref().map(|prov| {
        Arc::new(OnChainStateFetcher::new(
            prov.clone(),
            config.network.chain_id,
        ))
    });

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
    tracing::info!(
        target: "dex_simulator_cli::commands::listen",
        "PancakeSwap V2 监听器已启用: chain_id={}, bootstrap池子={} 个",
        config.network.chain_id,
        v2_bootstrap.len()
    );

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
        tracing::info!(
            target: "dex_simulator_cli::commands::listen",
            "PancakeSwap V3 监听器已启用: chain_id={}, bootstrap池子={} 个",
            config.network.chain_id,
            v3_bootstrap.len()
        );
    } else {
        tracing::info!(
            target: "dex_simulator_cli::commands::listen",
            "未启用 PancakeSwap V3 监听: 未配置 bootstrap 池子"
        );
    }

    let router = Arc::new(router_inner);

    let metadata =
        build_subscription_metadata(repository.clone(), &v2_bootstrap, &v3_bootstrap).await?;

    let router_sink = Arc::new(RouterSink::new(
        router.clone(),
        repository.clone(),
        Some(snapshot_store.clone()),
        v2_token_map.clone(),
        v3_pool_map.clone(),
        fetcher.clone(),
        metadata.pool_labels.clone(),
        config.network.chain_id,
    ));
    listener.subscribe(router_sink as Arc<dyn EventSink>).await;

    let logging_sink: Arc<dyn EventSink> =
        Arc::new(LoggingEventSink::new(metadata.pool_labels.clone()));
    listener.subscribe(logging_sink).await;

    let source: Arc<dyn EventSource> = if sample_events == 0 {
        build_live_event_source(config, &metadata, 0)
            .await
            .map_err(|err| anyhow!(err.to_string()))?
    } else {
        let events = build_mock_events(sample_events)?;
        Arc::new(MockEventSource::new(events))
    };
    listener.attach_source(source).await;

    listener.clone().start().await?;
    if sample_events == 0 {
        tracing::info!("实时监听已启动，按 Ctrl+C 结束...");
        if let Some(mut rx) = shutdown {
            let _ = rx.changed().await;
        } else {
            tokio::signal::ctrl_c()
                .await
                .map_err(|err| anyhow!(format!("等待 Ctrl+C 失败: {}", err)))?;
        }
    } else {
        let wait_ms = 300 * (sample_events + 4);
        sleep(Duration::from_millis(wait_ms)).await;
    }
    listener.clone().stop().await?;

    let snapshots = repository.list().await?;
    for snapshot in snapshots {
        tracing::info!("监听结束，池子状态: {:?}", snapshot);
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
        block_hash: None,
        block_timestamp: None,
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
        block_hash: None,
        block_timestamp: None,
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
        block_hash: None,
        block_timestamp: None,
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
            block_hash: None,
            block_timestamp: None,
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
            block_hash: None,
            block_timestamp: None,
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
    backfill_blocks: u64,
) -> Result<Arc<dyn EventSource>, EventListenerError> {
    let source_config = EthersEventSourceConfig {
        ws_endpoint: config.network.ws_endpoint.clone(),
        http_endpoint: config.network.http_endpoint.clone(),
        addresses: metadata.addresses.clone(),
        topics: metadata.topics.clone(),
        topic_kinds: metadata.topic_kinds.clone(),
        backfill_blocks,
        chunk_size: config.network.backfill_chunk_size.max(1),
        retry_interval: Duration::from_secs(config.network.ws_retry_secs.max(1)),
        channel_capacity: 2_048,
        warn_delay_ms: config.monitoring.event_delay_warn_ms,
        stats_event_interval: config.monitoring.event_stats_interval,
    };
    let source = EthersEventSource::connect(source_config).await?;
    Ok(source as Arc<dyn EventSource>)
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
        let block_hash = event
            .block_hash
            .map(|h| format!("{:#x}", h))
            .unwrap_or_else(|| "-".into());
        let block_time = event
            .block_timestamp
            .map(|ts| ts.to_string())
            .unwrap_or_else(|| "-".into());
        tracing::info!(
            "接收到事件: dex={}, kind={:?}, block={}, hash={}, time={}, tx={:#x}, log_index={}",
            label,
            event.kind,
            event.block_number,
            block_hash,
            block_time,
            event.transaction_hash,
            event.log_index
        );
        Ok(())
    }
}

struct RouterSink {
    router: Arc<DexEventRouter>,
    repository: Arc<dyn PoolRepository>,
    snapshot_store: Option<Arc<dyn SnapshotStore>>,
    v2_tokens: Option<HashMap<Address, (Asset, Asset)>>,
    v3_pools: HashMap<Address, V3BootstrapPool>,
    fetcher: Option<Arc<OnChainStateFetcher<Provider<Http>>>>,
    pool_labels: Arc<HashMap<Address, String>>,
    chain_id: u64,
}

impl RouterSink {
    fn new(
        router: Arc<DexEventRouter>,
        repository: Arc<dyn PoolRepository>,
        snapshot_store: Option<Arc<dyn SnapshotStore>>,
        v2_tokens: Option<HashMap<Address, (Asset, Asset)>>,
        v3_pools: HashMap<Address, V3BootstrapPool>,
        fetcher: Option<Arc<OnChainStateFetcher<Provider<Http>>>>,
        pool_labels: Arc<HashMap<Address, String>>,
        chain_id: u64,
    ) -> Self {
        Self {
            router,
            repository,
            snapshot_store,
            v2_tokens,
            v3_pools,
            fetcher,
            pool_labels,
            chain_id,
        }
    }

    async fn ensure_pool_initialized(
        &self,
        event: &EventEnvelope,
    ) -> Result<(), EventListenerError> {
        // PairCreated 会由处理器自行建立初始状态。
        if event.kind == EventKind::PairCreated {
            return Ok(());
        }

        match self.detect_pool_type(event) {
            Some(PoolType::PancakeV2) => self.ensure_v2_pool(event).await,
            Some(PoolType::PancakeV3) => self.ensure_v3_pool(event).await,
            _ => Ok(()),
        }
    }

    fn detect_pool_type(&self, event: &EventEnvelope) -> Option<PoolType> {
        if let Some(label) = self.pool_labels.get(&event.address) {
            return match label.as_str() {
                "pancake_v2" => Some(PoolType::PancakeV2),
                "pancake_v3" => Some(PoolType::PancakeV3),
                _ => None,
            };
        }
        if let Some(tokens) = &self.v2_tokens {
            if tokens.contains_key(&event.address) {
                return Some(PoolType::PancakeV2);
            }
        }
        if self.v3_pools.contains_key(&event.address) {
            return Some(PoolType::PancakeV3);
        }
        None
    }

    async fn ensure_v2_pool(&self, event: &EventEnvelope) -> Result<(), EventListenerError> {
        let id = PoolIdentifier {
            chain_namespace: ChainNamespace::Evm,
            chain_id: self.chain_id,
            dex: PoolType::PancakeV2.label(),
            address: event.address,
            pool_type: PoolType::PancakeV2,
        };
        if self
            .repository
            .get(&id)
            .await
            .map_err(to_internal)?
            .is_some()
        {
            return Ok(());
        }
        let fetcher = match &self.fetcher {
            Some(fetcher) => fetcher.clone(),
            None => {
                tracing::warn!(
                    "缺少 HTTP Provider，无法为池子 {:#x} 补齐状态",
                    event.address
                );
                return Ok(());
            }
        };
        let (token0, token1) = match &self.v2_tokens {
            Some(map) => match map.get(&event.address) {
                Some(pair) => pair.clone(),
                None => {
                    tracing::warn!("未找到池子 {:#x} 的 token 配置，跳过初始化", event.address);
                    return Ok(());
                }
            },
            None => {
                tracing::warn!(
                    "未配置 Pancake V2 token 信息，跳过池子 {:#x} 初始化",
                    event.address
                );
                return Ok(());
            }
        };
        let pool_info = V2BootstrapPool {
            pool: event.address,
            token0,
            token1,
        };
        let block = event.block_number.checked_sub(1);
        let snapshot = fetcher
            .fetch_v2_snapshot_at(&pool_info, block)
            .await
            .map_err(to_internal)?;
        self.repository
            .upsert(snapshot.clone())
            .await
            .map_err(to_internal)?;
        if let Some(store) = &self.snapshot_store {
            if let Err(err) = store.save(&snapshot).await {
                tracing::warn!("写入 V2 快照失败: pool={:#x}, 错误: {}", event.address, err);
            }
        }
        Ok(())
    }

    async fn ensure_v3_pool(&self, event: &EventEnvelope) -> Result<(), EventListenerError> {
        let id = PoolIdentifier {
            chain_namespace: ChainNamespace::Evm,
            chain_id: self.chain_id,
            dex: PoolType::PancakeV3.label(),
            address: event.address,
            pool_type: PoolType::PancakeV3,
        };
        if self
            .repository
            .get(&id)
            .await
            .map_err(to_internal)?
            .is_some()
        {
            return Ok(());
        }
        let fetcher = match &self.fetcher {
            Some(fetcher) => fetcher.clone(),
            None => {
                tracing::warn!(
                    "缺少 HTTP Provider，无法为 V3 池子 {:#x} 补齐状态",
                    event.address
                );
                return Ok(());
            }
        };
        let pool_info = match self.v3_pools.get(&event.address) {
            Some(info) => info.clone(),
            None => {
                tracing::warn!("未找到 V3 池子 {:#x} 的配置，跳过初始化", event.address);
                return Ok(());
            }
        };
        let block = event.block_number.checked_sub(1);
        let snapshot = fetcher
            .fetch_v3_snapshot_at(&pool_info, block)
            .await
            .map_err(to_internal)?;
        self.repository
            .upsert(snapshot.clone())
            .await
            .map_err(to_internal)?;
        if let Some(store) = &self.snapshot_store {
            if let Err(err) = store.save(&snapshot).await {
                tracing::warn!("写入 V3 快照失败: pool={:#x}, 错误: {}", event.address, err);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl EventSink for RouterSink {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        self.ensure_pool_initialized(&event).await?;
        self.router.dispatch(event).await
    }
}

fn to_internal<E: ToString>(err: E) -> EventListenerError {
    EventListenerError::Internal(err.to_string())
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
