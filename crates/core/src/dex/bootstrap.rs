use std::path::Path;
use std::str::FromStr;

use ethers::types::Address;
use serde::Deserialize;

use crate::types::Asset;

#[derive(Debug, Clone, Deserialize)]
pub struct TokenMeta {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
}

impl TokenMeta {
    fn to_asset(&self) -> Result<Asset, BootstrapError> {
        let address = Address::from_str(&self.address)
            .map_err(|err| BootstrapError::Address(err.to_string()))?;
        Ok(Asset {
            address,
            symbol: self.symbol.clone(),
            decimals: self.decimals,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct V2BootstrapPoolConfig {
    pub pool: String,
    pub token0: TokenMeta,
    pub token1: TokenMeta,
}

#[derive(Debug, Clone)]
pub struct V2BootstrapPool {
    pub pool: Address,
    pub token0: Asset,
    pub token1: Asset,
}

#[derive(Debug, Clone, Deserialize)]
pub struct V3BootstrapPoolConfig {
    pub pool: String,
    pub token0: TokenMeta,
    pub token1: TokenMeta,
}

#[derive(Debug, Clone)]
pub struct V3BootstrapPool {
    pub pool: Address,
    pub token0: Asset,
    pub token1: Asset,
}

#[derive(thiserror::Error, Debug)]
pub enum BootstrapError {
    #[error("配置读取失败: {0}")]
    Io(String),
    #[error("配置解析失败: {0}")]
    Parse(String),
    #[error("地址解析失败: {0}")]
    Address(String),
}

fn load_file(path: &Path) -> Result<String, BootstrapError> {
    std::fs::read_to_string(path).map_err(|err| BootstrapError::Io(err.to_string()))
}

pub fn load_v2_bootstrap<P: AsRef<Path>>(path: P) -> Result<Vec<V2BootstrapPool>, BootstrapError> {
    let raw = load_file(path.as_ref())?;
    let configs: Vec<V2BootstrapPoolConfig> =
        serde_yaml::from_str(&raw).map_err(|err| BootstrapError::Parse(err.to_string()))?;
    configs
        .into_iter()
        .map(|cfg| {
            let pool = Address::from_str(&cfg.pool)
                .map_err(|err| BootstrapError::Address(err.to_string()))?;
            Ok(V2BootstrapPool {
                pool,
                token0: cfg.token0.to_asset()?,
                token1: cfg.token1.to_asset()?,
            })
        })
        .collect()
}

pub fn load_v3_bootstrap<P: AsRef<Path>>(path: P) -> Result<Vec<V3BootstrapPool>, BootstrapError> {
    let raw = load_file(path.as_ref())?;
    let configs: Vec<V3BootstrapPoolConfig> =
        serde_yaml::from_str(&raw).map_err(|err| BootstrapError::Parse(err.to_string()))?;
    configs
        .into_iter()
        .map(|cfg| {
            let pool = Address::from_str(&cfg.pool)
                .map_err(|err| BootstrapError::Address(err.to_string()))?;
            Ok(V3BootstrapPool {
                pool,
                token0: cfg.token0.to_asset()?,
                token1: cfg.token1.to_asset()?,
            })
        })
        .collect()
}
