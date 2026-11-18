//! Typed predicates shared by Query and Subscribe.

use crate::{
    CodecError,
    record::{Decoder, Encoder},
};
use harmion_domain::{AttributeKey, AttributeValue, Number};

const MAGIC: &[u8; 4] = b"HQU\x01";
const MAX_CONDITION_DEPTH: usize = 16;
const MAX_QUERY_TERMS: usize = 128;
pub const MAX_QUERY_PAGE_ITEMS: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryCursor(pub [u8; 32]);

/// Canonical attribute bytes for structural equality keys in derived indexes.
pub fn attribute_key_bytes(value: &AttributeValue) -> Result<Vec<u8>, crate::CodecError> {
    let mut encoder = crate::record::Encoder::new();
    encoder.attribute(value, 0)?;
    Ok(encoder.finish())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
    Kind(String),
    LastEditedAt {
        operator: OrderOperator,
        /// Signed microseconds since the Unix epoch in UTC.
        utc_micros: i64,
    },
    Attribute {
        key: AttributeKey,
        predicate: AttributePredicate,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributePredicate {
    Exists,
    Eq(AttributeValue),
    Compare {
        operator: OrderOperator,
        value: OrderedValue,
    },
    Contains(AttributeValue),
    ContainsAny(Vec<AttributeValue>),
    ContainsAll(Vec<AttributeValue>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderOperator {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderedValue {
    Number(Number),
    String(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryRequest {
    Start { condition: Condition, limit: u32 },
    Next { cursor: QueryCursor, limit: u32 },
}

pub fn encode_request(request: &QueryRequest) -> Result<Vec<u8>, CodecError> {
    let mut encoder = Encoder::new();
    encoder.raw(MAGIC)?;
    match request {
        QueryRequest::Start { condition, limit } => {
            encoder.tag(0)?;
            encoder.raw(&limit.to_be_bytes())?;
            let mut budget = MAX_QUERY_TERMS;
            write_condition(&mut encoder, condition, 0, &mut budget)?;
        }
        QueryRequest::Next { cursor, limit } => {
            encoder.tag(1)?;
            encoder.raw(&limit.to_be_bytes())?;
            encoder.raw(&cursor.0)?;
        }
    }
    Ok(encoder.finish())
}

pub fn decode_request(bytes: &[u8]) -> Result<QueryRequest, CodecError> {
    let mut decoder = Decoder::new(bytes)?;
    if decoder.raw(MAGIC.len())? != MAGIC {
        return Err(CodecError::BadMagic);
    }
    let tag = decoder.tag()?;
    let limit = decoder.u32()?;
    let mut budget = MAX_QUERY_TERMS;
    let request = match tag {
        0 => QueryRequest::Start {
            condition: read_condition(&mut decoder, 0, &mut budget)?,
            limit,
        },
        1 => QueryRequest::Next {
            cursor: QueryCursor(decoder.fixed()?),
            limit,
        },
        _ => return Err(CodecError::InvalidTag),
    };
    decoder.finish()?;
    Ok(request)
}

fn debit(remaining: &mut usize, count: usize) -> Result<(), CodecError> {
    *remaining = remaining
        .checked_sub(count)
        .ok_or(CodecError::TooManyItems)?;
    Ok(())
}

fn write_condition(
    encoder: &mut Encoder,
    condition: &Condition,
    depth: usize,
    budget: &mut usize,
) -> Result<(), CodecError> {
    if depth > MAX_CONDITION_DEPTH {
        return Err(CodecError::TooDeep);
    }
    debit(budget, 1)?;
    match condition {
        Condition::And(items) | Condition::Or(items) => {
            encoder.tag(u8::from(matches!(condition, Condition::Or(_))))?;
            encoder.count(items.len())?;
            for item in items {
                write_condition(encoder, item, depth + 1, budget)?;
            }
            Ok(())
        }
        Condition::Not(inner) => {
            encoder.tag(2)?;
            write_condition(encoder, inner, depth + 1, budget)
        }
        Condition::Kind(kind) => {
            encoder.tag(3)?;
            encoder.string(kind)
        }
        Condition::LastEditedAt {
            operator,
            utc_micros,
        } => {
            encoder.tag(4)?;
            encoder.tag(order_tag(*operator))?;
            encoder.raw(&utc_micros.to_be_bytes())
        }
        Condition::Attribute { key, predicate } => {
            encoder.tag(5)?;
            encoder.string(&key.namespace)?;
            encoder.string(&key.key)?;
            write_attribute(encoder, predicate, budget)
        }
    }
}

fn read_condition(
    decoder: &mut Decoder<'_>,
    depth: usize,
    budget: &mut usize,
) -> Result<Condition, CodecError> {
    if depth > MAX_CONDITION_DEPTH {
        return Err(CodecError::TooDeep);
    }
    debit(budget, 1)?;
    Ok(match decoder.tag()? {
        tag @ (0 | 1) => {
            let items = (0..decoder.count()?)
                .map(|_| read_condition(decoder, depth + 1, budget))
                .collect::<Result<Vec<_>, _>>()?;
            if tag == 0 {
                Condition::And(items)
            } else {
                Condition::Or(items)
            }
        }
        2 => Condition::Not(Box::new(read_condition(decoder, depth + 1, budget)?)),
        3 => Condition::Kind(decoder.string()?),
        4 => Condition::LastEditedAt {
            operator: read_order(decoder)?,
            utc_micros: i64::from_be_bytes(decoder.fixed()?),
        },
        5 => Condition::Attribute {
            key: AttributeKey {
                namespace: decoder.string()?,
                key: decoder.string()?,
            },
            predicate: read_attribute(decoder, budget)?,
        },
        _ => return Err(CodecError::InvalidTag),
    })
}

fn order_tag(order: OrderOperator) -> u8 {
    match order {
        OrderOperator::Less => 0,
        OrderOperator::LessOrEqual => 1,
        OrderOperator::Greater => 2,
        OrderOperator::GreaterOrEqual => 3,
    }
}

fn read_order(decoder: &mut Decoder<'_>) -> Result<OrderOperator, CodecError> {
    match decoder.tag()? {
        0 => Ok(OrderOperator::Less),
        1 => Ok(OrderOperator::LessOrEqual),
        2 => Ok(OrderOperator::Greater),
        3 => Ok(OrderOperator::GreaterOrEqual),
        _ => Err(CodecError::InvalidTag),
    }
}

fn write_attribute(
    encoder: &mut Encoder,
    predicate: &AttributePredicate,
    budget: &mut usize,
) -> Result<(), CodecError> {
    match predicate {
        AttributePredicate::Exists => encoder.tag(0),
        AttributePredicate::Eq(value) | AttributePredicate::Contains(value) => {
            encoder.tag(if matches!(predicate, AttributePredicate::Eq(_)) {
                1
            } else {
                2
            })?;
            encoder.attribute(value, 0)
        }
        AttributePredicate::ContainsAny(values) | AttributePredicate::ContainsAll(values) => {
            encoder.tag(if matches!(predicate, AttributePredicate::ContainsAny(_)) {
                3
            } else {
                4
            })?;
            debit(budget, values.len())?;
            encoder.count(values.len())?;
            for value in values {
                encoder.attribute(value, 0)?;
            }
            Ok(())
        }
        AttributePredicate::Compare { operator, value } => {
            encoder.tag(5)?;
            encoder.tag(order_tag(*operator))?;
            match value {
                OrderedValue::Number(number) => {
                    encoder.tag(0)?;
                    encoder.string(&number.to_string())
                }
                OrderedValue::String(text) => {
                    encoder.tag(1)?;
                    encoder.string(text)
                }
            }
        }
    }
}

fn read_attribute(
    decoder: &mut Decoder<'_>,
    budget: &mut usize,
) -> Result<AttributePredicate, CodecError> {
    Ok(match decoder.tag()? {
        0 => AttributePredicate::Exists,
        1 => AttributePredicate::Eq(decoder.attribute(0)?),
        2 => AttributePredicate::Contains(decoder.attribute(0)?),
        tag @ (3 | 4) => {
            let count = decoder.count()?;
            debit(budget, count)?;
            let values = (0..count)
                .map(|_| decoder.attribute(0))
                .collect::<Result<Vec<_>, _>>()?;
            if tag == 3 {
                AttributePredicate::ContainsAny(values)
            } else {
                AttributePredicate::ContainsAll(values)
            }
        }
        5 => AttributePredicate::Compare {
            operator: read_order(decoder)?,
            value: match decoder.tag()? {
                0 => OrderedValue::Number(
                    decoder
                        .string()?
                        .parse()
                        .map_err(|_| CodecError::InvalidNumber)?,
                ),
                1 => OrderedValue::String(decoder.string()?),
                _ => return Err(CodecError::InvalidTag),
            },
        },
        _ => return Err(CodecError::InvalidTag),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_request_preserves_exact_literals_and_rejects_trailing_bytes() {
        let attribute = |predicate| Condition::Attribute {
            key: AttributeKey {
                namespace: "app/a".into(),
                key: "tags".into(),
            },
            predicate,
        };
        let exact = "9007199254740993".parse().unwrap();
        let request = QueryRequest::Start {
            condition: Condition::And(vec![
                Condition::Kind("wiki".into()),
                Condition::Not(Box::new(Condition::Or(vec![]))),
                Condition::LastEditedAt {
                    operator: OrderOperator::GreaterOrEqual,
                    utc_micros: i64::MIN,
                },
                attribute(AttributePredicate::Compare {
                    operator: OrderOperator::Less,
                    value: OrderedValue::Number(exact),
                }),
                attribute(AttributePredicate::ContainsAll(vec![
                    AttributeValue::Null,
                    AttributeValue::Object(
                        [(
                            "$serde_json::private::Number".into(),
                            AttributeValue::Bool(true),
                        )]
                        .into(),
                    ),
                ])),
            ]),
            limit: 4,
        };
        let mut bytes = encode_request(&request).unwrap();
        assert_eq!(decode_request(&bytes).unwrap(), request);
        bytes.push(0);
        assert_eq!(decode_request(&bytes), Err(CodecError::TrailingBytes));
        let next = QueryRequest::Next {
            cursor: QueryCursor([17; 32]),
            limit: 8,
        };
        assert_eq!(
            decode_request(&encode_request(&next).unwrap()).unwrap(),
            next
        );
    }
}
