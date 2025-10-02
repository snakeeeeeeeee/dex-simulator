use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ethers::providers::{Http, Middleware, Provider, Ws};
use ethers::types::{Address, BlockNumber, Filter, Log, ValueOrArray, H256};
use futures_util::StreamExt;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

use crate::event::{EventEnvelope, EventKind, EventListenerError, EventSource};

/// 基于 ethers-rs 的日志事件源配置。
#[derive(Debug, Clone)]
pub struct EthersEventSourceConfig {
    /// WebSocket 端点地址。
    pub ws_endpoint: String,
    /// HTTP 端点地址，用于回放与补齐日志。
    pub http_endpoint: String,
    /// 过滤的合约地址集合，若为空则不限制。
    pub addresses: Vec<Address>,
    /// 过滤的事件主题列表。
    pub topics: Vec<H256>,
    /// topic 与事件类型的映射，便于构造 EventKind。
    pub topic_kinds: HashMap<H256, EventKind>,
    /// 启动时补齐的回溯区块数量。
    pub backfill_blocks: u64,
    /// 单次回放的最大区块跨度。
    pub chunk_size: u64,
    /// 拉取失败后的等待时长。
    pub retry_interval: Duration,
    /// 内部队列容量。
    pub channel_capacity: usize,
    /// 事件延迟告警阈值（毫秒）。
    pub warn_delay_ms: u64,
    /// 日志统计间隔（多少事件输出一次）。
    pub stats_event_interval: u64,
}

impl Default for EthersEventSourceConfig {
    fn default() -> Self {
        Self {
            ws_endpoint: String::new(),
            http_endpoint: String::new(),
            addresses: Vec::new(),
            topics: Vec::new(),
            topic_kinds: HashMap::new(),
            backfill_blocks: 0,
            chunk_size: 1_000,
            retry_interval: Duration::from_secs(5),
            channel_capacity: 1_024,
            warn_delay_ms: 5_000,
            stats_event_interval: 100,
        }
    }
}

/// ethers-rs WebSocket/HTTP 结合的事件源实现。
pub struct EthersEventSource {
    inner: Arc<Inner>,
    receiver: Mutex<mpsc::Receiver<EventEnvelope>>,
    backfill_handle: Mutex<Option<JoinHandle<()>>>,
    ws_handle: Mutex<Option<JoinHandle<()>>>,
}

impl EthersEventSource {
    /// 建立链上事件源连接，并启动回放 + 实时订阅任务。
    pub async fn connect(config: EthersEventSourceConfig) -> Result<Arc<Self>, EventListenerError> {
        let http_provider = Provider::<Http>::try_from(config.http_endpoint.as_str())
            .map_err(|err| EventListenerError::Network(err.to_string()))?;
        let (sender, receiver) = mpsc::channel(config.channel_capacity);
        let inner = Arc::new(Inner {
            config,
            http: http_provider,
            sender,
            shutdown: AtomicBool::new(false),
            last_published_block: AtomicU64::new(0),
            event_counter: AtomicU64::new(0),
            last_event_ts: AtomicU64::new(0),
            ws_reconnects: AtomicU64::new(0),
        });
        let source = Arc::new(Self {
            inner: inner.clone(),
            receiver: Mutex::new(receiver),
            backfill_handle: Mutex::new(None),
            ws_handle: Mutex::new(None),
        });

        source.spawn_backfill().await;
        source.spawn_ws_loop().await;

        Ok(source)
    }

    async fn spawn_backfill(self: &Arc<Self>) {
        if self.inner.config.backfill_blocks == 0 {
            return;
        }
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            inner.run_backfill().await;
        });
        *self.backfill_handle.lock().await = Some(handle);
    }

    async fn spawn_ws_loop(self: &Arc<Self>) {
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            inner.run_ws_loop().await;
        });
        *self.ws_handle.lock().await = Some(handle);
    }
}

#[async_trait::async_trait]
impl EventSource for EthersEventSource {
    async fn next_event(&self) -> Result<Option<EventEnvelope>, EventListenerError> {
        let mut guard = self.receiver.lock().await;
        Ok(guard.recv().await)
    }

    async fn handle_disconnect(&self) -> Result<(), EventListenerError> {
        // 留空：内部任务会自动重连。
        Ok(())
    }
}

#[async_trait::async_trait]
impl EventSource for Arc<EthersEventSource> {
    async fn next_event(&self) -> Result<Option<EventEnvelope>, EventListenerError> {
        self.as_ref().next_event().await
    }

    async fn handle_disconnect(&self) -> Result<(), EventListenerError> {
        self.as_ref().handle_disconnect().await
    }
}

impl Drop for EthersEventSource {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
    }
}

struct Inner {
    config: EthersEventSourceConfig,
    http: Provider<Http>,
    sender: mpsc::Sender<EventEnvelope>,
    shutdown: AtomicBool,
    last_published_block: AtomicU64,
    event_counter: AtomicU64,
    last_event_ts: AtomicU64,
    ws_reconnects: AtomicU64,
}

impl Inner {
    fn base_filter(&self) -> Filter {
        let mut filter = Filter::new();
        if !self.config.addresses.is_empty() {
            filter = filter.address(ValueOrArray::Array(self.config.addresses.clone()));
        }
        if !self.config.topics.is_empty() {
            filter = filter.topic0(ValueOrArray::Array(self.config.topics.clone()));
        }
        filter
    }

    async fn run_backfill(self: Arc<Self>) {
        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let latest = match self.http.get_block_number().await {
                Ok(number) => number.as_u64(),
                Err(err) => {
                    log::warn!("HTTP 回放获取最新区块失败: {}", err);
                    sleep(self.config.retry_interval).await;
                    continue;
                }
            };
            if latest == 0 {
                sleep(self.config.retry_interval).await;
                continue;
            }
            let backfill_blocks = self.config.backfill_blocks.min(latest);
            let start = latest.saturating_sub(backfill_blocks);
            let chunk = self.config.chunk_size.max(1);
            log::info!(
                "开始回放日志: from_block={}, to_block={}, chunk={}",
                start,
                latest,
                chunk
            );
            let mut current = start;
            while current <= latest {
                if self.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let end = current.saturating_add(chunk - 1).min(latest);
                let mut filter = self.base_filter();
                filter = filter
                    .from_block(BlockNumber::Number(current.into()))
                    .to_block(BlockNumber::Number(end.into()));
                match self.http.get_logs(&filter).await {
                    Ok(logs) => {
                        for log in logs {
                            if let Err(err) = self.publish_log(log).await {
                                log::warn!("回放日志发送失败: {}", err);
                                if self.shutdown.load(Ordering::SeqCst) {
                                    break;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        log::warn!(
                            "回放日志请求失败: from={}, to={}, 错误: {}",
                            current,
                            end,
                            err
                        );
                        sleep(self.config.retry_interval).await;
                    }
                }
                sleep(Duration::from_millis(200)).await;
                if end == u64::MAX {
                    break;
                }
                current = end.saturating_add(1);
            }
            break;
        }
    }

    async fn run_ws_loop(self: Arc<Self>) {
        let base_retry = if self.config.retry_interval.is_zero() {
            Duration::from_secs(1)
        } else {
            self.config.retry_interval
        };
        let max_retry = base_retry
            .checked_mul(6)
            .unwrap_or_else(|| Duration::from_secs(base_retry.as_secs().max(1) * 6));
        let mut current_retry = base_retry;

        while !self.shutdown.load(Ordering::SeqCst) {
            let attempt = self.ws_reconnects.fetch_add(1, Ordering::SeqCst);
            if attempt > 0 {
                log::info!("尝试重连 WebSocket，第 {} 次", attempt);
            }
            let mut connected = false;
            match Provider::<Ws>::connect(self.config.ws_endpoint.as_str()).await {
                Ok(provider) => {
                    connected = true;
                    current_retry = base_retry;
                    log::info!("WebSocket 连接成功: {}", self.config.ws_endpoint);
                    match provider.subscribe_logs(&self.base_filter()).await {
                        Ok(mut stream) => {
                            while let Some(log) = stream.next().await {
                                if let Err(err) = self.publish_log(log).await {
                                    log::warn!("实时日志发送失败: {}", err);
                                    if self.shutdown.load(Ordering::SeqCst) {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            connected = false;
                            log::error!("日志订阅失败: {}", err);
                        }
                    }
                }
                Err(err) => {
                    log::error!("WebSocket 连接失败: {}", err);
                }
            }

            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }

            let wait = current_retry;
            log::info!("WebSocket 已断开，{} 秒后重试", wait.as_secs().max(1));
            sleep(wait).await;
            if connected {
                current_retry = base_retry;
            } else {
                current_retry = current_retry
                    .checked_mul(2)
                    .map(|d| if d > max_retry { max_retry } else { d })
                    .unwrap_or(max_retry);
            }
        }
    }

    async fn publish_log(&self, log: Log) -> Result<(), EventListenerError> {
        if log.removed.unwrap_or(false) {
            return Ok(());
        }
        let block_number = log.block_number.unwrap_or_default().as_u64();
        let kind = log
            .topics
            .get(0)
            .and_then(|topic| self.config.topic_kinds.get(topic))
            .cloned()
            .unwrap_or(EventKind::Unknown);
        let envelope = EventEnvelope {
            kind,
            block_number,
            transaction_hash: log.transaction_hash.unwrap_or_default(),
            log_index: log.log_index.unwrap_or_default().as_u64(),
            address: log.address,
            topics: log.topics.clone(),
            payload: log.data.clone().into(),
        };
        self.last_published_block
            .store(block_number, Ordering::SeqCst);
        self.sender
            .send(envelope)
            .await
            .map_err(|err| EventListenerError::Internal(err.to_string()))?;

        let count = self.event_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev_ms = self.last_event_ts.swap(now_ms, Ordering::SeqCst);
        let interval_ms = if prev_ms == 0 {
            0
        } else {
            now_ms.saturating_sub(prev_ms)
        };

        if self.config.warn_delay_ms > 0 && interval_ms > self.config.warn_delay_ms {
            log::warn!(
                "事件延迟告警: 距离上一事件 {} ms, 最新区块 {}",
                interval_ms,
                block_number
            );
        } else if self.config.stats_event_interval > 0
            && count % self.config.stats_event_interval == 0
        {
            let last_block = self.last_published_block.load(Ordering::SeqCst);
            let reconnects = self.ws_reconnects.load(Ordering::SeqCst);
            log::info!(
                "事件统计: 累计 {} 条, 最近间隔 {} ms, 最新区块 {}, 重连次数 {}",
                count,
                interval_ms,
                last_block,
                reconnects
            );
        }

        Ok(())
    }
}
