use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use ethers::types::Address;
use hex::encode_upper;
use tokio::sync::RwLock;

use crate::dex::pancake_v2::event::{PancakeV2Event, PancakeV2EventError};
use crate::dex::pancake_v2::state::PancakeV2PoolState;
use crate::dex::traits::DexEventHandler;
use crate::event::{EventEnvelope, EventKind, EventListenerError, EventSink};
use crate::state::repository::PoolRepository;
use crate::state::snapshot::SnapshotStore;
use crate::state::PoolSnapshot;
use crate::token_graph::TokenGraph;
use crate::types::{Asset, PoolIdentifier, PoolType};

/// PancakeSwap V2 事件处理器配置。
#[derive(Debug, Clone)]
pub struct PancakeV2Config {
    pub chain_id: u64,
    pub dex: String,
    pub factory_address: Address,
    pub default_decimals: u8,
    pub token_whitelist: Option<Vec<Address>>,
}

impl Default for PancakeV2Config {
    fn default() -> Self {
        Self {
            chain_id: 56,
            dex: "pancakeswap".into(),
            factory_address: Address::zero(),
            default_decimals: 18,
            token_whitelist: None,
        }
    }
}

/// PancakeSwap V2 事件处理器。
#[derive(Clone)]
pub struct PancakeV2EventHandler {
    config: PancakeV2Config,
    repository: Arc<dyn PoolRepository>,
    snapshot_store: Option<Arc<dyn SnapshotStore>>,
    token_graph: Arc<RwLock<TokenGraph>>,
    token_whitelist: Option<HashSet<Address>>,
}

impl PancakeV2EventHandler {
    pub fn new(
        config: PancakeV2Config,
        repository: Arc<dyn PoolRepository>,
        snapshot_store: Option<Arc<dyn SnapshotStore>>,
    ) -> Self {
        let whitelist = config
            .token_whitelist
            .as_ref()
            .map(|list| list.iter().cloned().collect());
        Self {
            config,
            repository,
            snapshot_store,
            token_graph: Arc::new(RwLock::new(TokenGraph::new())),
            token_whitelist: whitelist,
        }
    }

    async fn handle_parsed_event(&self, event: PancakeV2Event) -> Result<(), EventListenerError> {
        match event {
            PancakeV2Event::PairCreated {
                factory,
                token0,
                token1,
                pair,
                ..
            } => {
                if self.config.factory_address != Address::zero()
                    && self.config.factory_address != factory
                {
                    return Ok(());
                }
                let id = self.pool_identifier(pair);
                let token0_asset = self.build_asset(token0, "T0");
                let token1_asset = self.build_asset(token1, "T1");
                if !self
                    .should_track_pool(&[token0_asset.address, token1_asset.address])
                    .await
                {
                    log::info!(
                        "池子不在白名单连通图中，忽略: pair={:?}, token0={:?}, token1={:?}",
                        pair,
                        token0,
                        token1
                    );
                    return Ok(());
                }
                {
                    let mut graph = self.token_graph.write().await;
                    graph.add_pool(id.clone(), vec![token0_asset.clone(), token1_asset.clone()]);
                }
                let mut state = PancakeV2PoolState::new(id.clone(), token0_asset, token1_asset);
                state.set_reserves(0, 0);
                let snapshot = state
                    .to_snapshot()
                    .map_err(|err| EventListenerError::Internal(err.to_string()))?;
                self.persist_snapshot(snapshot).await
            }
            PancakeV2Event::Mint { pair, .. }
            | PancakeV2Event::Burn { pair, .. }
            | PancakeV2Event::Swap { pair, .. }
            | PancakeV2Event::Sync { pair, .. } => {
                let id = self.pool_identifier(pair);
                if let Some(snapshot) = self
                    .repository
                    .get(&id)
                    .await
                    .map_err(|err| EventListenerError::Internal(err.to_string()))?
                {
                    let mut state = PancakeV2PoolState::from_snapshot(&snapshot)
                        .map_err(|err| EventListenerError::Internal(err.to_string()))?;
                    state
                        .apply_pancake_event(&event)
                        .map_err(|err| EventListenerError::Internal(err.to_string()))?;
                    let snapshot = state
                        .to_snapshot()
                        .map_err(|err| EventListenerError::Internal(err.to_string()))?;
                    self.persist_snapshot(snapshot).await
                } else {
                    log::warn!("未找到池子状态，忽略事件 pair={:?}", pair);
                    Ok(())
                }
            }
        }
    }

    async fn should_track_pool(&self, tokens: &[Address]) -> bool {
        match &self.token_whitelist {
            None => true,
            Some(whitelist) => {
                if tokens.iter().any(|addr| whitelist.contains(addr)) {
                    return true;
                }
                let graph = self.token_graph.read().await;
                tokens
                    .iter()
                    .any(|addr| graph.is_connected_to_whitelist(*addr, whitelist))
            }
        }
    }

    async fn persist_snapshot(&self, snapshot: PoolSnapshot) -> Result<(), EventListenerError> {
        self.repository
            .upsert(snapshot.clone())
            .await
            .map_err(|err| EventListenerError::Internal(err.to_string()))?;
        if let Some(store) = &self.snapshot_store {
            store
                .save(&snapshot)
                .await
                .map_err(|err| EventListenerError::Internal(err.to_string()))?;
        }
        Ok(())
    }

    fn pool_identifier(&self, pair: Address) -> PoolIdentifier {
        PoolIdentifier {
            chain_id: self.config.chain_id,
            dex: self.config.dex.clone(),
            address: pair,
            pool_type: PoolType::PancakeV2,
        }
    }

    fn build_asset(&self, address: Address, prefix: &str) -> Asset {
        let suffix = encode_upper(&address.as_bytes()[0..3]);
        Asset {
            address,
            symbol: format!("{}-{}", prefix, suffix),
            decimals: self.config.default_decimals,
        }
    }

    /// 返回配置的工厂地址，若未设置则返回 None。
    pub fn factory_address(&self) -> Option<Address> {
        if self.config.factory_address.is_zero() {
            None
        } else {
            Some(self.config.factory_address)
        }
    }
}

#[async_trait]
impl EventSink for PancakeV2EventHandler {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        match PancakeV2Event::try_from(&event) {
            Ok(parsed) => self.handle_parsed_event(parsed).await,
            Err(PancakeV2EventError::Unsupported) => Ok(()),
            Err(err) => {
                log::warn!("PancakeV2 事件解析失败: {}", err);
                Err(EventListenerError::Decode(err.to_string()))
            }
        }
    }
}

#[async_trait]
impl DexEventHandler for PancakeV2EventHandler {
    async fn handle_envelope(&self, envelope: EventEnvelope) -> Result<(), EventListenerError> {
        self.handle_event(envelope).await
    }

    fn matches(&self, envelope: &EventEnvelope) -> bool {
        envelope.kind == EventKind::PairCreated
            || envelope.kind == EventKind::Mint
            || envelope.kind == EventKind::Swap
            || envelope.kind == EventKind::Burn
            || envelope.kind == EventKind::Sync
    }
}
