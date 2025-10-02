use std::path::Path;

use log::LevelFilter;
use log4rs::{
    append::console::ConsoleAppender,
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
    init_file,
};

use crate::config::LoggingConfig;

/// 初始化日志系统，优先使用外部配置文件，缺省退回控制台输出。
pub fn init_logging(config: &LoggingConfig) -> Result<(), LogInitError> {
    if let Some(path) = &config.config_file {
        let log_cfg_path = Path::new(path);
        if log_cfg_path.exists() {
            init_file(log_cfg_path, Default::default())
                .map_err(|err| LogInitError::ExternalConfig(err.to_string()))?;
            return Ok(());
        }
    }

    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d} {l} {t} - {m}{n}")))
        .build();

    let log_level: LevelFilter = config.level.parse().unwrap_or(LevelFilter::Info);

    let log_config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .build(Root::builder().appender("stdout").build(log_level))
        .map_err(|err| LogInitError::Internal(err.to_string()))?;

    log4rs::init_config(log_config)
        .map(|_| ())
        .map_err(|err| LogInitError::Internal(err.to_string()))
}

/// 日志初始化相关错误。
#[derive(thiserror::Error, Debug)]
pub enum LogInitError {
    #[error("外部日志配置初始化失败: {0}")]
    ExternalConfig(String),
    #[error("日志系统初始化失败: {0}")]
    Internal(String),
}
