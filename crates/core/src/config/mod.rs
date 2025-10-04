use std::path::Path;

use config::{Config, ConfigError, File, FileFormat};
use once_cell::sync::OnceCell;
use serde::Deserialize;

static GLOBAL_CONFIG: OnceCell<AppConfig> = OnceCell::new();

/// 全局应用配置，后续可扩展更多字段。
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub logging: LoggingConfig,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub tokens: TokenConfig,
    #[serde(default)]
    pub dex: DexConfig,
    #[serde(default)]
    pub monitoring: MonitoringConfig,
}

/// 链路相关配置，包含 RPC 节点与订阅设置。
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    pub chain_id: u64,
    pub ws_endpoint: String,
    pub http_endpoint: String,
    pub event_backfill_blocks: u64,
    #[serde(default = "default_ws_retry_secs")]
    pub ws_retry_secs: u64,
    #[serde(default = "default_backfill_chunk_size")]
    pub backfill_chunk_size: u64,
}

/// 日志配置，兼容 log4rs。
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    pub config_file: Option<String>,
    pub level: String,
}

/// 运行时配置：并发度、快照等策略。
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub max_concurrency: usize,
    pub snapshot_path: String,
}

/// Token 相关配置。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TokenConfig {
    pub whitelist: Option<Vec<String>>,
}

/// DEX 相关配置。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DexConfig {
    pub pancake_v2_bootstrap: Option<String>,
    pub pancake_v3_bootstrap: Option<String>,
}

/// 监控相关配置。
#[derive(Debug, Clone, Deserialize)]
pub struct MonitoringConfig {
    #[serde(default = "default_warn_delay_ms")]
    pub event_delay_warn_ms: u64,
    #[serde(default = "default_stats_interval")]
    pub event_stats_interval: u64,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            event_delay_warn_ms: default_warn_delay_ms(),
            event_stats_interval: default_stats_interval(),
        }
    }
}

const fn default_warn_delay_ms() -> u64 {
    5_000
}

const fn default_stats_interval() -> u64 {
    100
}

const fn default_ws_retry_secs() -> u64 {
    5
}

const fn default_backfill_chunk_size() -> u64 {
    500
}

/// 从配置文件加载应用配置，支持 YAML/TOML 等格式。
pub fn load_config<P: AsRef<Path>>(path: P) -> Result<AppConfig, ConfigError> {
    let path_ref = path.as_ref();
    let mut builder = Config::builder();

    builder = match path_ref.extension().and_then(|ext| ext.to_str()) {
        Some("yaml") | Some("yml") => {
            builder.add_source(File::from(path_ref).format(FileFormat::Yaml))
        }
        Some("json") => builder.add_source(File::from(path_ref).format(FileFormat::Json)),
        Some("toml") | Some("conf") => {
            builder.add_source(File::from(path_ref).format(FileFormat::Toml))
        }
        _ => builder.add_source(File::from(path_ref)),
    };

    builder.build()?.try_deserialize()
}

/// 初始化全局配置，仅在首次调用时生效。
pub fn init_global_config(config: AppConfig) -> Result<(), AppConfigError> {
    GLOBAL_CONFIG
        .set(config)
        .map_err(|_| AppConfigError::AlreadyInitialized)
}

/// 获取全局配置引用。
pub fn get_config() -> Result<&'static AppConfig, AppConfigError> {
    GLOBAL_CONFIG.get().ok_or(AppConfigError::Uninitialized)
}

/// 配置相关错误定义。
#[derive(thiserror::Error, Debug)]
pub enum AppConfigError {
    #[error("配置已初始化")]
    AlreadyInitialized,
    #[error("配置尚未初始化")]
    Uninitialized,
}
