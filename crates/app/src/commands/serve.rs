use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::shared::SharedObjects;
use dex_simulator_core::config::AppConfig;

use super::listen;
use super::simulate::{build_pancake_v2_response, build_pancake_v3_response};

const DEFAULT_V2_AMOUNT: u128 = 1_000_000_000_000_000_000u128;
const DEFAULT_V3_AMOUNT: u128 = 1_000_000_000_000_000u128;

#[derive(Clone)]
struct AppState {
    config: Arc<AppConfig>,
    shared: Option<SharedObjects>,
}

#[derive(Debug, Deserialize)]
struct PancakeV2Request {
    #[serde(default = "default_pool_id")]
    pool_id: String,
    #[serde(default = "default_v2_amount")]
    amount: u128,
}

#[derive(Debug, Deserialize)]
struct PancakeV3Request {
    #[serde(default = "default_v3_amount")]
    amount: u128,
    #[serde(default)]
    reverse: bool,
}

fn default_pool_id() -> String {
    "sample".into()
}

fn default_v2_amount() -> u128 {
    DEFAULT_V2_AMOUNT
}

fn default_v3_amount() -> u128 {
    DEFAULT_V3_AMOUNT
}

struct HttpError {
    status: StatusCode,
    message: String,
}

impl HttpError {
    fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    fn internal(err: impl ToString) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": self.message,
        });
        (self.status, Json(body)).into_response()
    }
}

type HttpResult<T> = std::result::Result<T, HttpError>;

pub async fn serve(config: &AppConfig, dry_run: bool) -> Result<()> {
    if dry_run {
        tracing::info!("[dry-run] 服务未真正启动");
        return Ok(());
    }

    let config_arc = Arc::new(config.clone());
    let listener_enabled = config.services.listener.enabled;
    let http_enabled = config.services.http.enabled;
    let metrics_enabled = config.services.metrics.enabled;

    if !listener_enabled && !http_enabled && !metrics_enabled {
        tracing::warn!("未启用任何服务组件，直接退出");
        return Ok(());
    }

    if metrics_enabled {
        tracing::warn!("Metrics 服务尚未实现，暂不启动");
    }

    let shared = if listener_enabled || http_enabled {
        Some(SharedObjects::in_memory())
    } else {
        None
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut handles = Vec::new();

    if listener_enabled {
        let cfg = config_arc.clone();
        let shared_clone = shared.clone();
        let rx = shutdown_rx.clone();
        let sample_events = config.services.listener.sample_events;
        handles.push(tokio::spawn(async move {
            listen::listen_with_state(cfg.as_ref(), sample_events, shared_clone, Some(rx)).await
        }));
    }

    if http_enabled {
        let cfg = config_arc.clone();
        let shared_clone = shared.clone();
        let rx = shutdown_rx.clone();
        handles.push(tokio::spawn(run_http_server(cfg, shared_clone, rx)));
    }

    tracing::info!("服务组件启动完成，按 Ctrl+C 退出");
    tokio::signal::ctrl_c().await?;
    tracing::info!("收到 Ctrl+C，开始关闭服务组件");
    let _ = shutdown_tx.send(true);

    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::error!("组件退出异常: {}", err),
            Err(err) => tracing::error!("组件任务 Join 失败: {}", err),
        }
    }

    Ok(())
}

async fn simulate_pancake_v2_handler(
    Path(chain): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<PancakeV2Request>,
) -> HttpResult<Json<Value>> {
    ensure_chain_supported(&chain)?;
    let shared_ref = state.shared.as_ref();
    let output = build_pancake_v2_response(
        state.config.as_ref(),
        shared_ref,
        &payload.pool_id,
        payload.amount,
    )
    .await
    .map_err(HttpError::internal)?;
    Ok(Json(output))
}

async fn simulate_pancake_v3_handler(
    Path(chain): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<PancakeV3Request>,
) -> HttpResult<Json<Value>> {
    ensure_chain_supported(&chain)?;
    let shared_ref = state.shared.as_ref();
    let output = build_pancake_v3_response(shared_ref, payload.amount, payload.reverse)
        .await
        .map_err(HttpError::internal)?;
    Ok(Json(output))
}

fn ensure_chain_supported(chain: &str) -> HttpResult<()> {
    if chain.eq_ignore_ascii_case("bsc") {
        Ok(())
    } else {
        Err(HttpError::bad_request(format!("暂不支持链: {}", chain)))
    }
}

async fn run_http_server(
    config: Arc<AppConfig>,
    shared: Option<SharedObjects>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let addr: SocketAddr = config
        .runtime
        .http_bind
        .parse()
        .map_err(|err| anyhow!("HTTP 监听地址解析失败: {}", err))?;

    let state = AppState {
        config: config.clone(),
        shared,
    };

    let app = Router::new()
        .route(
            "/simulate/{chain}/pancake-v2",
            post(simulate_pancake_v2_handler),
        )
        .route(
            "/simulate/{chain}/pancake-v3",
            post(simulate_pancake_v3_handler),
        )
        .with_state(state);

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|err| anyhow!("绑定 HTTP 地址失败: {}", err))?;
    tracing::info!("HTTP 服务启动: http://{}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|err| anyhow!("HTTP 服务异常退出: {}", err))?;
    Ok(())
}
