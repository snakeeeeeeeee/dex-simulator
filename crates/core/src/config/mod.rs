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
    #[serde(default)]
    pub services: ServicesConfig,
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

/// 日志配置，可通过 tracing EnvFilter 调节输出等级。
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    pub config_file: Option<String>,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_directory")]
    pub directory: String,
    #[serde(default = "default_log_size_mb")]
    pub max_size_mb: u64,
    #[serde(default = "default_log_retention_hours")]
    pub retention_hours: u64,
}

/// 运行时配置：并发度、快照等策略。
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub max_concurrency: usize,
    pub snapshot_path: String,
    #[serde(default = "default_http_bind")]
    pub http_bind: String,
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

fn default_http_bind() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_directory() -> String {
    "logs".to_string()
}

const fn default_log_size_mb() -> u64 {
    100
}

const fn default_log_retention_hours() -> u64 {
    24
}

/// 服务编排配置。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServicesConfig {
    #[serde(default)]
    pub listener: ListenerServiceConfig,
    #[serde(default)]
    pub http: HttpServiceConfig,
    #[serde(default)]
    pub metrics: ServiceToggle,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerServiceConfig {
    #[serde(default = "default_disabled")]
    pub enabled: bool,
    #[serde(default)]
    pub sample_events: u64,
}

impl Default for ListenerServiceConfig {
    fn default() -> Self {
        Self {
            enabled: default_disabled(),
            sample_events: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpServiceConfig {
    #[serde(default = "default_disabled")]
    pub enabled: bool,
}

impl Default for HttpServiceConfig {
    fn default() -> Self {
        Self {
            enabled: default_disabled(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceToggle {
    #[serde(default = "default_disabled")]
    pub enabled: bool,
}

impl Default for ServiceToggle {
    fn default() -> Self {
        Self {
            enabled: default_disabled(),
        }
    }
}

const fn default_disabled() -> bool {
    false
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
