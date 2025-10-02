use std::sync::Arc;

use crate::dex::traits::DexEventHandler;
use crate::event::{EventEnvelope, EventListenerError};

/// DEX 事件路由器，根据 handler 的匹配规则分发事件。
#[derive(Default)]
pub struct DexEventRouter {
    handlers: Vec<Arc<dyn DexEventHandler>>,
}

impl DexEventRouter {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    pub fn with_handlers(handlers: Vec<Arc<dyn DexEventHandler>>) -> Self {
        Self { handlers }
    }

    pub fn register(&mut self, handler: Arc<dyn DexEventHandler>) {
        self.handlers.push(handler);
    }

    pub async fn dispatch(&self, envelope: EventEnvelope) -> Result<(), EventListenerError> {
        for handler in &self.handlers {
            if handler.matches(&envelope) {
                handler.handle_envelope(envelope.clone()).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ethers::types::Address;

    use crate::event::{EventKind, EventListenerError};

    struct DummyHandler {
        addr: Address,
        counter: tokio::sync::Mutex<u32>,
    }

    #[async_trait]
    impl DexEventHandler for DummyHandler {
        async fn handle_envelope(
            &self,
            _envelope: EventEnvelope,
        ) -> Result<(), EventListenerError> {
            let mut guard = self.counter.lock().await;
            *guard += 1;
            Ok(())
        }

        fn matches(&self, envelope: &EventEnvelope) -> bool {
            envelope.address == self.addr
        }
    }

    #[tokio::test]
    async fn test_dispatch() {
        let handler = Arc::new(DummyHandler {
            addr: Address::repeat_byte(0x11),
            counter: tokio::sync::Mutex::new(0),
        });
        let router = DexEventRouter::with_handlers(vec![handler.clone()]);

        let envelope = EventEnvelope {
            kind: EventKind::Unknown,
            block_number: 1,
            transaction_hash: Default::default(),
            log_index: 0,
            address: Address::repeat_byte(0x11),
            topics: vec![],
            payload: Default::default(),
        };

        router.dispatch(envelope).await.unwrap();
        assert_eq!(*handler.counter.lock().await, 1);
    }
}
