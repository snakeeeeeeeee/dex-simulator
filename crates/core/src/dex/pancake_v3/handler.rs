use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use ethers::types::Address;
use serde::Deserialize;
use tokio::fs;

use crate::dex::pancake_v3::event::{PancakeV3Event, PancakeV3EventError};
use crate::dex::pancake_v3::state::PancakeV3PoolState;
use crate::dex::traits::DexEventHandler;
use crate::event::{EventEnvelope, EventListenerError, EventSink};
use crate::state::repository::PoolRepository;
use crate::state::snapshot::SnapshotStore;
use crate::state::PoolSnapshot;
use crate::state::PoolState;
use crate::types::{Asset, PoolIdentifier, PoolType};

/// PancakeSwap V3 事件处理器配置。
#[derive(Debug, Clone)]
pub struct PancakeV3Config {
    pub chain_id: u64,
    pub dex: String,
    pub default_decimals: u8,
}

impl Default for PancakeV3Config {
    fn default() -> Self {
        Self {
            chain_id: 56,
            dex: "pancakeswap".into(),
            default_decimals: 18,
        }
    }
}

/// V3 池子元数据。
#[derive(Debug, Clone)]
pub struct PoolBootstrap {
    pub pool: Address,
    pub token0: Asset,
    pub token1: Asset,
}

#[derive(Debug, Deserialize)]
pub struct PoolBootstrapConfig {
    pub pool: String,
    pub token0: TokenMeta,
    pub token1: TokenMeta,
}

#[derive(Debug, Deserialize)]
pub struct TokenMeta {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
}

/// PancakeSwap V3 事件处理器。
#[derive(Clone)]
pub struct PancakeV3EventHandler {
    config: PancakeV3Config,
    repository: Arc<dyn PoolRepository>,
    snapshot_store: Option<Arc<dyn SnapshotStore>>,
    tokens: HashMap<Address, (Asset, Asset)>,
}

impl PancakeV3EventHandler {
    pub fn new(
        config: PancakeV3Config,
        repository: Arc<dyn PoolRepository>,
        snapshot_store: Option<Arc<dyn SnapshotStore>>,
        bootstraps: Vec<PoolBootstrap>,
    ) -> Self {
        let mut tokens = HashMap::new();
        for item in bootstraps {
            tokens.insert(item.pool, (item.token0, item.token1));
        }
        Self {
            config,
            repository,
            snapshot_store,
            tokens,
        }
    }

    pub async fn from_config_file<P: AsRef<std::path::Path>>(
        path: P,
        config: PancakeV3Config,
        repository: Arc<dyn PoolRepository>,
        snapshot_store: Option<Arc<dyn SnapshotStore>>,
    ) -> Result<Self, EventListenerError> {
        let raw = fs::read_to_string(path)
            .await
            .map_err(|err| EventListenerError::Internal(err.to_string()))?;
        let configs: Vec<PoolBootstrapConfig> = serde_yaml::from_str(&raw)
            .map_err(|err| EventListenerError::Internal(err.to_string()))?;
        let mut bootstraps = Vec::new();
        for item in configs {
            let pool = Address::from_str(&item.pool)
                .map_err(|err| EventListenerError::Internal(err.to_string()))?;
            let token0 = Asset {
                address: Address::from_str(&item.token0.address)
                    .map_err(|err| EventListenerError::Internal(err.to_string()))?,
                symbol: item.token0.symbol,
                decimals: item.token0.decimals,
            };
            let token1 = Asset {
                address: Address::from_str(&item.token1.address)
                    .map_err(|err| EventListenerError::Internal(err.to_string()))?,
                symbol: item.token1.symbol,
                decimals: item.token1.decimals,
            };
            bootstraps.push(PoolBootstrap {
                pool,
                token0,
                token1,
            });
        }
        Ok(Self::new(config, repository, snapshot_store, bootstraps))
    }

    async fn handle_parsed_event(
        &self,
        pool: Address,
        event: PancakeV3Event,
    ) -> Result<(), EventListenerError> {
        let id = self.pool_identifier(pool);
        let mut state = match self
            .repository
            .get(&id)
            .await
            .map_err(|err| EventListenerError::Internal(err.to_string()))?
        {
            Some(snapshot) => PancakeV3PoolState::from_snapshot(&snapshot)
                .map_err(|err| EventListenerError::Internal(err.to_string()))?,
            None => self.bootstrap_state(pool).await?,
        };

        state
            .update_from_event(&event)
            .map_err(|err| EventListenerError::Internal(err.to_string()))?;

        let snapshot = state.snapshot();
        self.persist_snapshot(snapshot).await
    }

    async fn bootstrap_state(
        &self,
        pool: Address,
    ) -> Result<PancakeV3PoolState, EventListenerError> {
        let (token0, token1) = self
            .tokens
            .get(&pool)
            .cloned()
            .ok_or_else(|| EventListenerError::Internal("缺少池子元数据".into()))?;
        let state = PancakeV3PoolState::new(self.pool_identifier(pool), token0, token1);
        Ok(state)
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

    fn pool_identifier(&self, pool: Address) -> PoolIdentifier {
        PoolIdentifier {
            chain_id: self.config.chain_id,
            dex: self.config.dex.clone(),
            address: pool,
            pool_type: PoolType::PancakeV3,
        }
    }

    /// 返回当前追踪的 V3 池子地址集合。
    pub fn tracked_pools(&self) -> Vec<Address> {
        self.tokens.keys().cloned().collect()
    }
}

#[async_trait]
impl EventSink for PancakeV3EventHandler {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        match PancakeV3Event::try_from(&event) {
            Ok(parsed) => self.handle_parsed_event(event.address, parsed).await,
            Err(PancakeV3EventError::MissingTopics) => Ok(()),
            Err(err) => Err(EventListenerError::Decode(err.to_string())),
        }
    }
}

#[async_trait]
impl DexEventHandler for PancakeV3EventHandler {
    async fn handle_envelope(&self, envelope: EventEnvelope) -> Result<(), EventListenerError> {
        self.handle_event(envelope).await
    }

    fn matches(&self, envelope: &EventEnvelope) -> bool {
        self.tokens.contains_key(&envelope.address)
    }
}
