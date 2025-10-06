use std::convert::TryFrom;

use ethers::abi::{decode, ParamType, Token};
use ethers::types::{Address, H256, I256, U256};
use ethers::utils::keccak256;
use once_cell::sync::Lazy;

use crate::event::EventEnvelope;

const INITIALIZE_SIGNATURE: &str = "Initialize(uint160,int24)";
const MINT_SIGNATURE: &str = "Mint(address,address,int24,int24,uint128,uint256,uint256)";
const BURN_SIGNATURE: &str = "Burn(address,int24,int24,uint128,uint256,uint256)";
const SWAP_SIGNATURE: &str = "Swap(address,address,int256,int256,uint160,uint128,int24)";
const COLLECT_SIGNATURE: &str = "Collect(address,address,int24,int24,uint128,uint128)";

static INITIALIZE_TOPIC: Lazy<H256> = Lazy::new(|| topic(INITIALIZE_SIGNATURE));
static MINT_TOPIC: Lazy<H256> = Lazy::new(|| topic(MINT_SIGNATURE));
static BURN_TOPIC: Lazy<H256> = Lazy::new(|| topic(BURN_SIGNATURE));
static SWAP_TOPIC: Lazy<H256> = Lazy::new(|| topic(SWAP_SIGNATURE));
static COLLECT_TOPIC: Lazy<H256> = Lazy::new(|| topic(COLLECT_SIGNATURE));

fn topic(signature: &str) -> H256 {
    H256::from_slice(&keccak256(signature.as_bytes()))
}

fn topic_to_address(topic: &H256) -> Address {
    Address::from_slice(&topic.as_bytes()[12..])
}

/// PancakeSwap V3 事件枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PancakeV3Event {
    Initialize {
        sqrt_price_x96: U256,
        tick: i32,
    },
    Mint {
        sender: Address,
        owner: Address,
        tick_lower: i32,
        tick_upper: i32,
        liquidity: U256,
        amount0: U256,
        amount1: U256,
    },
    Burn {
        owner: Address,
        tick_lower: i32,
        tick_upper: i32,
        liquidity: U256,
        amount0: U256,
        amount1: U256,
    },
    Swap {
        sender: Address,
        recipient: Address,
        amount0: I256,
        amount1: I256,
        sqrt_price_x96: U256,
        liquidity: U256,
        tick: i32,
    },
    Collect {
        sender: Address,
        recipient: Address,
        tick_lower: i32,
        tick_upper: i32,
        amount0: U256,
        amount1: U256,
    },
    Unknown,
}

impl TryFrom<&EventEnvelope> for PancakeV3Event {
    type Error = PancakeV3EventError;

    fn try_from(event: &EventEnvelope) -> Result<Self, Self::Error> {
        if event.topics.is_empty() {
            return Err(PancakeV3EventError::MissingTopics);
        }
        let topic0 = event.topics[0];
        if topic0 == *INITIALIZE_TOPIC {
            parse_initialize(event)
        } else if topic0 == *MINT_TOPIC {
            parse_mint(event)
        } else if topic0 == *BURN_TOPIC {
            parse_burn(event)
        } else if topic0 == *SWAP_TOPIC {
            parse_swap(event)
        } else if topic0 == *COLLECT_TOPIC {
            parse_collect(event)
        } else {
            Ok(PancakeV3Event::Unknown)
        }
    }
}

/// 事件解析错误。
#[derive(thiserror::Error, Debug)]
pub enum PancakeV3EventError {
    #[error("缺少事件主题(topic)")]
    MissingTopics,
    #[error("事件数据解析失败: {0}")]
    Decode(String),
}

fn parse_initialize(event: &EventEnvelope) -> Result<PancakeV3Event, PancakeV3EventError> {
    let decoded = decode(&[ParamType::Uint(160), ParamType::Int(24)], &event.payload)
        .map_err(|err| PancakeV3EventError::Decode(err.to_string()))?;
    let sqrt_price = expect_uint(&decoded[0], 160)?;
    let tick = expect_int(&decoded[1], 24)?;
    Ok(PancakeV3Event::Initialize {
        sqrt_price_x96: sqrt_price,
        tick,
    })
}

fn parse_mint(event: &EventEnvelope) -> Result<PancakeV3Event, PancakeV3EventError> {
    if event.topics.len() < 4 {
        return Err(PancakeV3EventError::MissingTopics);
    }
    let owner = topic_to_address(&event.topics[1]);
    let tick_lower = topic_to_int24(&event.topics[2])?;
    let tick_upper = topic_to_int24(&event.topics[3])?;
    let decoded = decode(
        &[
            ParamType::Address,
            ParamType::Uint(128),
            ParamType::Uint(256),
            ParamType::Uint(256),
        ],
        &event.payload,
    )
    .map_err(|err| PancakeV3EventError::Decode(err.to_string()))?;
    let sender = decoded[0]
        .clone()
        .into_address()
        .ok_or_else(|| PancakeV3EventError::Decode("sender address 缺失".into()))?;
    Ok(PancakeV3Event::Mint {
        sender,
        owner,
        tick_lower,
        tick_upper,
        liquidity: expect_uint(&decoded[1], 128)?,
        amount0: expect_uint(&decoded[2], 256)?,
        amount1: expect_uint(&decoded[3], 256)?,
    })
}

fn parse_burn(event: &EventEnvelope) -> Result<PancakeV3Event, PancakeV3EventError> {
    if event.topics.len() < 4 {
        return Err(PancakeV3EventError::MissingTopics);
    }
    let owner = topic_to_address(&event.topics[1]);
    let tick_lower = topic_to_int24(&event.topics[2])?;
    let tick_upper = topic_to_int24(&event.topics[3])?;
    let decoded = decode(
        &[
            ParamType::Uint(128),
            ParamType::Uint(256),
            ParamType::Uint(256),
        ],
        &event.payload,
    )
    .map_err(|err| PancakeV3EventError::Decode(err.to_string()))?;
    Ok(PancakeV3Event::Burn {
        owner,
        tick_lower,
        tick_upper,
        liquidity: expect_uint(&decoded[0], 128)?,
        amount0: expect_uint(&decoded[1], 256)?,
        amount1: expect_uint(&decoded[2], 256)?,
    })
}

fn parse_swap(event: &EventEnvelope) -> Result<PancakeV3Event, PancakeV3EventError> {
    if event.topics.len() < 3 {
        return Err(PancakeV3EventError::MissingTopics);
    }
    let sender = topic_to_address(&event.topics[1]);
    let recipient = topic_to_address(&event.topics[2]);
    let decoded = decode(
        &[
            ParamType::Int(256),
            ParamType::Int(256),
            ParamType::Uint(160),
            ParamType::Uint(128),
            ParamType::Int(24),
        ],
        &event.payload,
    )
    .map_err(|err| PancakeV3EventError::Decode(err.to_string()))?;
    Ok(PancakeV3Event::Swap {
        sender,
        recipient,
        amount0: expect_int_256(&decoded[0])?,
        amount1: expect_int_256(&decoded[1])?,
        sqrt_price_x96: expect_uint(&decoded[2], 160)?,
        liquidity: expect_uint(&decoded[3], 128)?,
        tick: expect_int(&decoded[4], 24)?,
    })
}

fn parse_collect(event: &EventEnvelope) -> Result<PancakeV3Event, PancakeV3EventError> {
    if event.topics.len() < 4 {
        return Err(PancakeV3EventError::MissingTopics);
    }
    let sender = topic_to_address(&event.topics[1]);
    let tick_lower = topic_to_int24(&event.topics[2])?;
    let tick_upper = topic_to_int24(&event.topics[3])?;
    let decoded = decode(
        &[
            ParamType::Address,
            ParamType::Uint(128),
            ParamType::Uint(128),
        ],
        &event.payload,
    )
    .map_err(|err| PancakeV3EventError::Decode(err.to_string()))?;
    Ok(PancakeV3Event::Collect {
        sender,
        recipient: decoded[0]
            .clone()
            .into_address()
            .ok_or_else(|| PancakeV3EventError::Decode("recipient address 缺失".into()))?,
        tick_lower,
        tick_upper,
        amount0: expect_uint(&decoded[1], 128)?,
        amount1: expect_uint(&decoded[2], 128)?,
    })
}

fn expect_uint(token: &Token, bits: usize) -> Result<U256, PancakeV3EventError> {
    token
        .clone()
        .into_uint()
        .ok_or_else(|| PancakeV3EventError::Decode(format!("期待 Uint({})", bits)))
}

fn expect_int(token: &Token, bits: usize) -> Result<i32, PancakeV3EventError> {
    token
        .clone()
        .into_int()
        .ok_or_else(|| PancakeV3EventError::Decode(format!("期待 Int({})", bits)))
        .map(|int| {
            let signed = I256::from_raw(int);
            signed.as_i32()
        })
}

fn expect_int_256(token: &Token) -> Result<I256, PancakeV3EventError> {
    token
        .clone()
        .into_int()
        .ok_or_else(|| PancakeV3EventError::Decode("期待 Int256".into()))
        .map(I256::from_raw)
}

fn topic_to_int24(topic: &H256) -> Result<i32, PancakeV3EventError> {
    let value = U256::from_big_endian(topic.as_bytes());
    let signed = I256::from_raw(value);
    Ok(signed.as_i32())
}

/// 返回事件签名 topic 集合，便于统一订阅。
pub fn event_topics() -> &'static [H256; 5] {
    static TOPICS: Lazy<[H256; 5]> = Lazy::new(|| {
        [
            *INITIALIZE_TOPIC,
            *MINT_TOPIC,
            *BURN_TOPIC,
            *SWAP_TOPIC,
            *COLLECT_TOPIC,
        ]
    });
    &TOPICS
}

pub fn initialize_topic() -> H256 {
    *INITIALIZE_TOPIC
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

pub fn collect_topic() -> H256 {
    *COLLECT_TOPIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::abi::encode;

    fn encode_int24(value: i32) -> Token {
        Token::Int(I256::from(value).into_raw())
    }

    #[test]
    fn test_parse_initialize() {
        let payload = encode(&[Token::Uint(U256::from(123u64)), encode_int24(-5)]);
        let event = EventEnvelope {
            kind: crate::event::EventKind::Unknown,
            block_number: 0,
            block_hash: None,
            block_timestamp: None,
            transaction_hash: H256::zero(),
            log_index: 0,
            address: Address::zero(),
            topics: vec![*INITIALIZE_TOPIC],
            payload: payload.into(),
        };
        let parsed = PancakeV3Event::try_from(&event).expect("parse failed");
        match parsed {
            PancakeV3Event::Initialize {
                sqrt_price_x96,
                tick,
            } => {
                assert_eq!(sqrt_price_x96, U256::from(123u64));
                assert_eq!(tick, -5);
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }
}
