pub mod queue;
pub mod source;
pub mod store;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ethers::types::{Address, Bytes, TransactionReceipt, H256};
use hex::encode as hex_encode;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use self::queue::QueueError;
use self::store::EventStoreError;

/// 事件类型枚举，覆盖 PancakeSwap 不同事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Swap,
    Mint,
    Burn,
    PairCreated,
    Sync,
    Collect,
    Unknown,
}

/// 标准化的链上事件封装，便于统一处理。
#[derive(Debug, Clone)]
pub struct EventEnvelope {
    pub kind: EventKind,
    pub block_number: u64,
    pub block_hash: Option<H256>,
    pub block_timestamp: Option<u64>,
    pub transaction_hash: H256,
    pub log_index: u64,
    pub address: Address,
    pub topics: Vec<H256>,
    pub payload: Bytes,
}

/// 事件监听器统一接口。
#[async_trait]
pub trait EventListener: Send + Sync {
    /// 启动监听流程，内部应处理重连与回放，错误需外抛。
    async fn start(&self) -> Result<(), EventListenerError>;

    /// 关闭监听流程，确保资源释放。
    async fn stop(&self) -> Result<(), EventListenerError>;
}

/// 事件处理结果回调接口，供消费者实现。
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn handle_event(&self, event: EventEnvelope) -> Result<(), EventListenerError>;
}

/// 事件源抽象，封装链上订阅或回放逻辑。
#[async_trait]
pub trait EventSource: Send + Sync {
    async fn next_event(&self) -> Result<Option<EventEnvelope>, EventListenerError>;

    /// 当连接断开时调用，缺省为立即恢复。
    async fn handle_disconnect(&self) -> Result<(), EventListenerError> {
        Ok(())
    }
}

/// 基于本地队列与存储的监听器实现，阶段 1 先行使用。
pub struct LocalEventListener {
    queue: Arc<dyn queue::EventQueue>,
    store: Arc<dyn store::EventStore>,
    source: Mutex<Option<Arc<dyn EventSource>>>,
    running: AtomicBool,
    subscribers: Arc<RwLock<Vec<Arc<dyn EventSink>>>>,
    pump_handle: Mutex<Option<JoinHandle<()>>>,
    fetch_handle: Mutex<Option<JoinHandle<()>>>,
}

impl LocalEventListener {
    pub fn new(queue: Arc<dyn queue::EventQueue>, store: Arc<dyn store::EventStore>) -> Self {
        Self {
            queue,
            store,
            source: Mutex::new(None),
            running: AtomicBool::new(false),
            subscribers: Arc::new(RwLock::new(Vec::new())),
            pump_handle: Mutex::new(None),
            fetch_handle: Mutex::new(None),
        }
    }

    /// 注入事件源，可在启动前或运行中调用。
    pub async fn attach_source(&self, source: Arc<dyn EventSource>) {
        let mut guard = self.source.lock().await;
        *guard = Some(source);
    }

    /// 注册一个事件消费端。
    pub async fn subscribe(&self, sink: Arc<dyn EventSink>) {
        let mut guard = self.subscribers.write().await;
        guard.push(sink);
    }

    /// 手动发布事件到队列与存储。
    pub async fn publish(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        self.store.append(&event).await?;
        self.queue.enqueue(event).await?;
        Ok(())
    }

    async fn dispatch(&self, event: EventEnvelope) -> Result<(), EventListenerError> {
        let guard = self.subscribers.read().await;
        for sink in guard.iter() {
            sink.handle_event(event.clone()).await?;
        }
        Ok(())
    }

    async fn pump_loop(self: Arc<Self>) {
        while self.running.load(Ordering::SeqCst) {
            match self.queue.dequeue().await {
                Some(event) => {
                    let event_clone = event.clone();
                    if let Err(err) = self.dispatch(event).await {
                        log::error!(
                            target: "dex_simulator_core::event",
                            "事件分发失败: {err}; event: {}",
                            serde_json::to_string_pretty(&Self::event_json(&event_clone))
                                .unwrap_or_else(|_| "<serde_json_error>".to_string())
                        );
                        log::error!(target: "dex_simulator_core::event", "错误详情: {err:?}");
                    }
                }
                None => {
                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
    }

    fn event_json(event: &EventEnvelope) -> serde_json::Value {
        json!({
            "kind": format!("{:?}", event.kind),
            "block_number": event.block_number,
            "block_hash": event.block_hash.map(|h| format!("{:#x}", h)),
            "block_timestamp": event.block_timestamp,
            "transaction_hash": format!("{:#x}", event.transaction_hash),
            "log_index": event.log_index,
            "address": format!("{:#x}", event.address),
            "topics": event.topics.iter().map(|t| format!("{:#x}", t)).collect::<Vec<_>>(),
            "payload": format!("0x{}", hex_encode(&event.payload)),
        })
    }

    async fn fetch_loop(self: Arc<Self>, source: Arc<dyn EventSource>) {
        while self.running.load(Ordering::SeqCst) {
            match source.next_event().await {
                Ok(Some(event)) => {
                    if let Err(err) = self.publish(event).await {
                        log::error!("写入事件失败: {}", err);
                    }
                }
                Ok(None) => {
                    sleep(Duration::from_millis(200)).await;
                }
                Err(err) => {
                    log::warn!("事件源异常: {}，尝试重连", err);
                    if let Err(retry_err) = source.handle_disconnect().await {
                        log::error!("事件源重连失败: {}", retry_err);
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    async fn stop_handles(&self) {
        if let Some(handle) = self.pump_handle.lock().await.take() {
            if let Err(err) = handle.await {
                log::warn!("Pump 任务结束异常: {}", err);
            }
        }
        if let Some(handle) = self.fetch_handle.lock().await.take() {
            if let Err(err) = handle.await {
                log::warn!("Fetch 任务结束异常: {}", err);
            }
        }
    }
}

#[async_trait]
impl EventListener for Arc<LocalEventListener> {
    async fn start(&self) -> Result<(), EventListenerError> {
        if self.running.swap(true, Ordering::SeqCst) {
            log::warn!("事件监听器已在运行，忽略重复启动");
            return Ok(());
        }

        let pump_self = Arc::clone(self);
        let pump_handle = tokio::spawn(async move {
            pump_self.pump_loop().await;
        });
        *self.pump_handle.lock().await = Some(pump_handle);

        if let Some(source) = self.source.lock().await.clone() {
            let fetch_self = Arc::clone(self);
            let fetch_handle = tokio::spawn(async move {
                fetch_self.fetch_loop(source).await;
            });
            *self.fetch_handle.lock().await = Some(fetch_handle);
        } else {
            log::info!("未配置事件源，监听器仅处理手工注入事件");
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), EventListenerError> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        self.queue.shutdown().await;
        self.stop_handles().await;
        Ok(())
    }
}

/// 监听器相关错误。
#[derive(thiserror::Error, Debug)]
pub enum EventListenerError {
    #[error("网络异常: {0}")]
    Network(String),
    #[error("解码失败: {0}")]
    Decode(String),
    #[error("内部错误: {0}")]
    Internal(String),
    #[error("队列错误: {0}")]
    Queue(String),
    #[error("存储错误: {0}")]
    Storage(String),
}

impl From<QueueError> for EventListenerError {
    fn from(err: QueueError) -> Self {
        EventListenerError::Queue(err.to_string())
    }
}

impl From<EventStoreError> for EventListenerError {
    fn from(err: EventStoreError) -> Self {
        EventListenerError::Storage(err.to_string())
    }
}

/// 事件回放上下文信息。
#[derive(Debug, Clone)]
pub struct ReplayContext {
    pub from_block: u64,
    pub to_block: u64,
    pub receipts: Vec<TransactionReceipt>,
    pub total_logs: usize,
}

/// 通用的事件回放接口。
#[async_trait]
pub trait EventReplayer: Send + Sync {
    async fn replay(&self, context: ReplayContext) -> Result<(), EventListenerError>;
}

/// 可注入测试事件的内存事件源。
#[derive(Debug, Default)]
pub struct MockEventSource {
    events: Mutex<VecDeque<EventEnvelope>>,
}

impl MockEventSource {
    pub fn new(events: Vec<EventEnvelope>) -> Self {
        Self {
            events: Mutex::new(events.into()),
        }
    }

    pub async fn push_events(&self, events: Vec<EventEnvelope>) {
        let mut guard = self.events.lock().await;
        for event in events {
            guard.push_back(event);
        }
    }
}

#[async_trait]
impl EventSource for MockEventSource {
    async fn next_event(&self) -> Result<Option<EventEnvelope>, EventListenerError> {
        let mut guard = self.events.lock().await;
        Ok(guard.pop_front())
    }
}
