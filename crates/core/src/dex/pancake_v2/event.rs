use std::convert::TryFrom;

use ethers::abi::{decode, ParamType, Token};
use ethers::types::{Address, H256, U256};
use ethers::utils::keccak256;
use once_cell::sync::Lazy;

use crate::event::EventEnvelope;

const PAIR_CREATED_SIGNATURE: &str = "PairCreated(address,address,address,uint256)";
const MINT_SIGNATURE: &str = "Mint(address,uint256,uint256)";
const BURN_SIGNATURE: &str = "Burn(address,uint256,uint256,address)";
const SWAP_SIGNATURE: &str = "Swap(address,uint256,uint256,uint256,uint256,address)";
const SYNC_SIGNATURE: &str = "Sync(uint112,uint112)";

static PAIR_CREATED_TOPIC: Lazy<H256> = Lazy::new(|| topic(PAIR_CREATED_SIGNATURE));
static MINT_TOPIC: Lazy<H256> = Lazy::new(|| topic(MINT_SIGNATURE));
static BURN_TOPIC: Lazy<H256> = Lazy::new(|| topic(BURN_SIGNATURE));
static SWAP_TOPIC: Lazy<H256> = Lazy::new(|| topic(SWAP_SIGNATURE));
static SYNC_TOPIC: Lazy<H256> = Lazy::new(|| topic(SYNC_SIGNATURE));

fn topic(signature: &str) -> H256 {
    H256::from_slice(&keccak256(signature.as_bytes()))
}

fn topic_to_address(topic: &H256) -> Address {
    let bytes = topic.as_bytes();
    Address::from_slice(&bytes[12..])
}

/// PancakeSwap V2 事件枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PancakeV2Event {
    PairCreated {
        factory: Address,
        token0: Address,
        token1: Address,
        pair: Address,
        all_pairs_length: U256,
    },
    Mint {
        pair: Address,
        sender: Address,
        amount0: U256,
        amount1: U256,
    },
    Burn {
        pair: Address,
        sender: Address,
        to: Address,
        amount0: U256,
        amount1: U256,
    },
    Swap {
        pair: Address,
        sender: Address,
        to: Address,
        amount0_in: U256,
        amount1_in: U256,
        amount0_out: U256,
        amount1_out: U256,
    },
    Sync {
        pair: Address,
        reserve0: U256,
        reserve1: U256,
    },
}

/// 事件解析错误。
#[derive(thiserror::Error, Debug)]
pub enum PancakeV2EventError {
    #[error("缺少事件主题(topic)")]
    MissingTopics,
    #[error("事件数据解析失败: {0}")]
    Decode(String),
    #[error("不支持的事件类型")]
    Unsupported,
}

impl TryFrom<&EventEnvelope> for PancakeV2Event {
    type Error = PancakeV2EventError;

    fn try_from(event: &EventEnvelope) -> Result<Self, Self::Error> {
        if event.topics.is_empty() {
            return Err(PancakeV2EventError::MissingTopics);
        }
        let topic0 = event.topics[0];
        if topic0 == *PAIR_CREATED_TOPIC {
            parse_pair_created(event)
        } else if topic0 == *MINT_TOPIC {
            parse_mint(event)
        } else if topic0 == *BURN_TOPIC {
            parse_burn(event)
        } else if topic0 == *SWAP_TOPIC {
            parse_swap(event)
        } else if topic0 == *SYNC_TOPIC {
            parse_sync(event)
        } else {
            Err(PancakeV2EventError::Unsupported)
        }
    }
}

fn parse_pair_created(event: &EventEnvelope) -> Result<PancakeV2Event, PancakeV2EventError> {
    if event.topics.len() < 3 {
        return Err(PancakeV2EventError::MissingTopics);
    }
    let token0 = topic_to_address(&event.topics[1]);
    let token1 = topic_to_address(&event.topics[2]);
    let decoded = decode(&[ParamType::Address, ParamType::Uint(256)], &event.payload)
        .map_err(|err| PancakeV2EventError::Decode(err.to_string()))?;
    let pair = decoded[0]
        .clone()
        .into_address()
        .ok_or_else(|| PancakeV2EventError::Decode("pair address 缺失".into()))?;
    let length = decoded[1]
        .clone()
        .into_uint()
        .ok_or_else(|| PancakeV2EventError::Decode("allPairsLength 缺失".into()))?;
    Ok(PancakeV2Event::PairCreated {
        factory: event.address,
        token0,
        token1,
        pair,
        all_pairs_length: length,
    })
}

fn parse_mint(event: &EventEnvelope) -> Result<PancakeV2Event, PancakeV2EventError> {
    if event.topics.len() < 2 {
        return Err(PancakeV2EventError::MissingTopics);
    }
    let sender = topic_to_address(&event.topics[1]);
    let decoded = decode(
        &[ParamType::Uint(256), ParamType::Uint(256)],
        &event.payload,
    )
    .map_err(|err| PancakeV2EventError::Decode(err.to_string()))?;
    Ok(PancakeV2Event::Mint {
        pair: event.address,
        sender,
        amount0: expect_uint(&decoded[0])?,
        amount1: expect_uint(&decoded[1])?,
    })
}

fn parse_burn(event: &EventEnvelope) -> Result<PancakeV2Event, PancakeV2EventError> {
    if event.topics.len() < 3 {
        return Err(PancakeV2EventError::MissingTopics);
    }
    let sender = topic_to_address(&event.topics[1]);
    let to = topic_to_address(&event.topics[2]);
    let decoded = decode(
        &[ParamType::Uint(256), ParamType::Uint(256)],
        &event.payload,
    )
    .map_err(|err| PancakeV2EventError::Decode(err.to_string()))?;
    Ok(PancakeV2Event::Burn {
        pair: event.address,
        sender,
        to,
        amount0: expect_uint(&decoded[0])?,
        amount1: expect_uint(&decoded[1])?,
    })
}

fn parse_swap(event: &EventEnvelope) -> Result<PancakeV2Event, PancakeV2EventError> {
    if event.topics.len() < 3 {
        return Err(PancakeV2EventError::MissingTopics);
    }
    let sender = topic_to_address(&event.topics[1]);
    let to = topic_to_address(&event.topics[2]);
    let decoded = decode(
        &[
            ParamType::Uint(256),
            ParamType::Uint(256),
            ParamType::Uint(256),
            ParamType::Uint(256),
        ],
        &event.payload,
    )
    .map_err(|err| PancakeV2EventError::Decode(err.to_string()))?;
    Ok(PancakeV2Event::Swap {
        pair: event.address,
        sender,
        to,
        amount0_in: expect_uint(&decoded[0])?,
        amount1_in: expect_uint(&decoded[1])?,
        amount0_out: expect_uint(&decoded[2])?,
        amount1_out: expect_uint(&decoded[3])?,
    })
}

fn parse_sync(event: &EventEnvelope) -> Result<PancakeV2Event, PancakeV2EventError> {
    let decoded = decode(
        &[ParamType::Uint(112), ParamType::Uint(112)],
        &event.payload,
    )
    .map_err(|err| PancakeV2EventError::Decode(err.to_string()))?;
    Ok(PancakeV2Event::Sync {
        pair: event.address,
        reserve0: expect_uint(&decoded[0])?,
        reserve1: expect_uint(&decoded[1])?,
    })
}

fn expect_uint(token: &Token) -> Result<U256, PancakeV2EventError> {
    token
        .clone()
        .into_uint()
        .ok_or_else(|| PancakeV2EventError::Decode("Uint 参数缺失".into()))
}

/// 计算事件签名 topic 常量。
pub fn event_topics() -> &'static [H256; 5] {
    static TOPICS: Lazy<[H256; 5]> = Lazy::new(|| {
        [
            *PAIR_CREATED_TOPIC,
            *MINT_TOPIC,
            *BURN_TOPIC,
            *SWAP_TOPIC,
            *SYNC_TOPIC,
        ]
    });
    &TOPICS
}

/// 对外暴露常量，便于构造模拟事件。
pub fn pair_created_topic() -> H256 {
    *PAIR_CREATED_TOPIC
}

pub fn mint_topic() -> H256 {
    *MINT_TOPIC
}

pub fn burn_topic() -> H256 {
    *BURN_TOPIC
}

pub fn swap_topic() -> H256 {
    *SWAP_TOPIC
}

pub fn sync_topic() -> H256 {
    *SYNC_TOPIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::abi::encode;
    use ethers::types::Bytes;

    fn mk_topic(addr: Address) -> H256 {
        let mut bytes = [0u8; 32];
        bytes[12..].copy_from_slice(addr.as_bytes());
        H256::from(bytes)
    }

    #[test]
    fn test_parse_swap_event() {
        let pair = Address::random();
        let sender = Address::random();
        let to = Address::random();
        let payload = encode(&[
            Token::Uint(U256::from(100u64)),
            Token::Uint(U256::from(0u64)),
            Token::Uint(U256::from(0u64)),
            Token::Uint(U256::from(45u64)),
        ]);
        let event = EventEnvelope {
            kind: crate::event::EventKind::Swap,
            block_number: 1,
            transaction_hash: H256::zero(),
            log_index: 0,
            address: pair,
            topics: vec![swap_topic(), mk_topic(sender), mk_topic(to)],
            payload: Bytes::from(payload),
        };
        let parsed = PancakeV2Event::try_from(&event).expect("swap 解析失败");
        match parsed {
            PancakeV2Event::Swap {
                pair: parsed_pair,
                amount0_in,
                amount1_out,
                ..
            } => {
                assert_eq!(parsed_pair, pair);
                assert_eq!(amount0_in, U256::from(100u64));
                assert_eq!(amount1_out, U256::from(45u64));
            }
            other => panic!("解析结果错误: {:?}", other),
        }
    }
}
