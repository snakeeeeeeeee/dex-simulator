//! 本地 DEX 模拟器核心库，提供事件监听、状态管理、路径搜索与模拟执行等能力。

pub mod config;
pub mod dex;
pub mod event;
pub mod logging;
pub mod path;
pub mod simulation;
pub mod state;
pub mod syncer;
pub mod token_graph;
pub mod types;
pub mod serde_utils;
