use std::sync::Arc;

use async_trait::async_trait;
use dex_simulator_core::dex::router::DexEventRouter;
use dex_simulator_core::dex::traits::DexEventHandler;
use dex_simulator_core::event::{EventEnvelope, EventKind, EventListenerError};
use ethers::types::Address;
use tokio::sync::Mutex;

struct CountingHandler {
    address: Address,
    counter: Mutex<u32>,
}

#[async_trait]
impl DexEventHandler for CountingHandler {
    async fn handle_envelope(&self, _envelope: EventEnvelope) -> Result<(), EventListenerError> {
        let mut guard = self.counter.lock().await;
        *guard += 1;
        Ok(())
    }

    fn matches(&self, envelope: &EventEnvelope) -> bool {
        envelope.address == self.address
    }
}

#[tokio::test]
async fn test_router_dispatch_v2_and_v3() {
    let addr_v2 = Address::repeat_byte(0x11);
    let addr_v3 = Address::repeat_byte(0x22);

    let handler_v2 = Arc::new(CountingHandler {
        address: addr_v2,
        counter: Mutex::new(0),
    });
    let handler_v3 = Arc::new(CountingHandler {
        address: addr_v3,
        counter: Mutex::new(0),
    });

    let router = DexEventRouter::with_handlers(vec![handler_v2.clone(), handler_v3.clone()]);

    let envelope_v2 = EventEnvelope {
        kind: EventKind::Swap,
        block_number: 1,
        block_hash: None,
        block_timestamp: None,
        transaction_hash: Default::default(),
        log_index: 0,
        address: addr_v2,
        topics: vec![],
        payload: Default::default(),
    };
    let envelope_v3 = EventEnvelope {
        kind: EventKind::Swap,
        block_number: 1,
        block_hash: None,
        block_timestamp: None,
        transaction_hash: Default::default(),
        log_index: 0,
        address: addr_v3,
        topics: vec![],
        payload: Default::default(),
    };

    router.dispatch(envelope_v2).await.unwrap();
    router.dispatch(envelope_v3).await.unwrap();

    assert_eq!(*handler_v2.counter.lock().await, 1);
    assert_eq!(*handler_v3.counter.lock().await, 1);
}
