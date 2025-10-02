use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};

use super::{EventEnvelope, EventListenerError};

/// 事件队列接口，负责为下游提供顺序事件。
#[async_trait]
pub trait EventQueue: Send + Sync {
    async fn enqueue(&self, event: EventEnvelope) -> Result<(), QueueError>;
    async fn dequeue(&self) -> Option<EventEnvelope>;
    async fn shutdown(&self);
}

/// 队列错误定义。
#[derive(thiserror::Error, Debug)]
pub enum QueueError {
    #[error("队列已关闭")]
    Closed,
    #[error("队列内部错误: {0}")]
    Internal(String),
}

/// 基于内存的简易事件队列，提供阶段 1 使用。
#[derive(Debug)]
pub struct InMemoryEventQueue {
    inner: Mutex<VecDeque<EventEnvelope>>,
    notify: Notify,
    closed: AtomicBool,
}

impl InMemoryEventQueue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    async fn do_enqueue(&self, event: EventEnvelope) -> Result<(), QueueError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(QueueError::Closed);
        }
        let mut guard = self.inner.lock().await;
        guard.push_back(event);
        drop(guard);
        self.notify.notify_one();
        Ok(())
    }

    async fn do_dequeue(&self) -> Option<EventEnvelope> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                let mut guard = self.inner.lock().await;
                return guard.pop_front();
            }
            {
                let mut guard = self.inner.lock().await;
                if let Some(ev) = guard.pop_front() {
                    return Some(ev);
                }
            }
            self.notify.notified().await;
        }
    }

    async fn do_shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

#[async_trait]
impl EventQueue for InMemoryEventQueue {
    async fn enqueue(&self, event: EventEnvelope) -> Result<(), QueueError> {
        self.do_enqueue(event).await
    }

    async fn dequeue(&self) -> Option<EventEnvelope> {
        self.do_dequeue().await
    }

    async fn shutdown(&self) {
        self.do_shutdown().await;
    }
}

#[async_trait]
impl EventQueue for Arc<InMemoryEventQueue> {
    async fn enqueue(&self, event: EventEnvelope) -> Result<(), QueueError> {
        self.as_ref().do_enqueue(event).await
    }

    async fn dequeue(&self) -> Option<EventEnvelope> {
        self.as_ref().do_dequeue().await
    }

    async fn shutdown(&self) {
        self.as_ref().do_shutdown().await;
    }
}

impl From<EventListenerError> for QueueError {
    fn from(err: EventListenerError) -> Self {
        QueueError::Internal(err.to_string())
    }
}
