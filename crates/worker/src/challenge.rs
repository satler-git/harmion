//! Durable challenge state over the Durable Object host KV boundary.
//!
//! Same semantics as the native SQLite adapter: fail-closed wall-clock reversal,
//! deterministic replay of an identical live challenge, bounded reclaim, atomic
//! unused -> used consumption, generation fencing and integer token buckets.
use crate::kv::{HostError, KvHost, KvTransaction};
use harmion_domain::NodeId;
use harmion_node::Error;
use harmion_node::https_peer::{
    ChallengeClock, ChallengeRecord, ChallengeStore, ChallengeTransaction,
};
use harmion_protocol::https_peer::{Commitment, PayloadClass};

const STATE_KEY: &[u8] = b"https/state";
const CHALLENGE_PREFIX: &[u8] = b"https/challenge/";
const RATE_PREFIX: &[u8] = b"https/rate/";
const ID: usize = 32;

pub struct WorkerChallenges<H: KvHost> {
    host: H,
    /// Runtime-only non-decreasing clamp. The host must supply wall time that
    /// already advances monotonically; a stalled clock never extends a live row.
    last_now: Option<i64>,
}

impl<H: KvHost> WorkerChallenges<H> {
    pub fn new(host: H) -> Self {
        Self {
            host,
            last_now: None,
        }
    }
    pub fn host(&self) -> &H {
        &self.host
    }
    /// Simulates an ordinary activation: durable rows stay, runtime state resets.
    pub fn into_host(self) -> H {
        self.host
    }
    /// Physical durability gate, called by the host before releasing an ACK.
    pub fn sync(&mut self) -> Result<(), HostError> {
        self.host.sync()
    }
}

impl<H: KvHost> ChallengeStore for WorkerChallenges<H> {
    fn challenge_transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn ChallengeTransaction) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let Self { host, last_now } = self;
        host.transaction(|kv| {
            let mut state = WorkerState {
                kv,
                last_now: *last_now,
            };
            let value = f(&mut state).map_err(|error| HostError(format!("{error:?}")))?;
            *last_now = state.last_now;
            Ok(value)
        })
        .map_err(|error| Error::Storage(error.0))
    }
}

struct WorkerState<'a> {
    kv: &'a mut dyn KvTransaction,
    last_now: Option<i64>,
}

fn host(error: HostError) -> Error {
    Error::Storage(error.0)
}
fn id(bytes: &[u8]) -> Result<[u8; ID], Error> {
    bytes.try_into().map_err(|_| Error::CorruptLog)
}
fn challenge_key(generation: &[u8; ID], requester: NodeId, nonce: &[u8; ID]) -> Vec<u8> {
    let mut key = Vec::with_capacity(CHALLENGE_PREFIX.len() + ID * 3);
    key.extend_from_slice(CHALLENGE_PREFIX);
    key.extend_from_slice(generation);
    key.extend_from_slice(&requester.0);
    key.extend_from_slice(nonce);
    key
}
fn rate_key(scope: NodeId) -> Vec<u8> {
    let mut key = Vec::with_capacity(RATE_PREFIX.len() + ID);
    key.extend_from_slice(RATE_PREFIX);
    key.extend_from_slice(&scope.0);
    key
}
fn encode_challenge(record: &ChallengeRecord) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(178);
    bytes.extend_from_slice(&record.challenge_nonce);
    bytes.extend_from_slice(&record.challenge_request_hash);
    bytes.extend_from_slice(&record.challenge_hash);
    bytes.push(record.commitment.class as u8);
    bytes.extend_from_slice(&record.commitment.inner_length.to_be_bytes());
    bytes.extend_from_slice(&record.commitment.inner_digest);
    bytes.extend_from_slice(&record.ttl_ms.to_be_bytes());
    bytes.extend_from_slice(&record.expires_at.to_be_bytes());
    bytes.push(u8::from(record.used));
    bytes
}
fn decode_challenge(
    requester: NodeId,
    client_nonce: [u8; ID],
    generation: [u8; ID],
    bytes: &[u8],
) -> Result<ChallengeRecord, Error> {
    if bytes.len() != 146 {
        return Err(Error::CorruptLog);
    }
    Ok(ChallengeRecord {
        requester,
        client_nonce,
        generation,
        challenge_nonce: id(&bytes[0..32])?,
        challenge_request_hash: id(&bytes[32..64])?,
        challenge_hash: id(&bytes[64..96])?,
        commitment: Commitment {
            class: match bytes[96] {
                0 => PayloadClass::Data,
                1 => PayloadClass::Evidence,
                _ => return Err(Error::CorruptLog),
            },
            inner_length: u32::from_be_bytes(bytes[97..101].try_into().unwrap()),
            inner_digest: id(&bytes[101..133])?,
        },
        ttl_ms: u32::from_be_bytes(bytes[133..137].try_into().unwrap()),
        expires_at: i64::from_be_bytes(bytes[137..145].try_into().unwrap()),
        used: match bytes[145] {
            0 => false,
            1 => true,
            _ => return Err(Error::CorruptLog),
        },
    })
}

impl WorkerState<'_> {
    fn read_state(&mut self) -> Result<Option<([u8; ID], i64)>, Error> {
        let Some(bytes) = self.kv.get(STATE_KEY).map_err(host)? else {
            return Ok(None);
        };
        if bytes.len() != ID + 8 {
            return Err(Error::CorruptLog);
        }
        Ok(Some((
            id(&bytes[..ID])?,
            i64::from_be_bytes(bytes[ID..].try_into().unwrap()),
        )))
    }
    fn write_state(&mut self, generation: [u8; ID], high_water: i64) -> Result<(), Error> {
        let mut bytes = Vec::with_capacity(ID + 8);
        bytes.extend_from_slice(&generation);
        bytes.extend_from_slice(&high_water.to_be_bytes());
        self.kv.put(STATE_KEY, bytes).map_err(host)
    }
}

impl ChallengeTransaction for WorkerState<'_> {
    fn clock(
        &mut self,
        observed_wall_micros: i64,
        random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
    ) -> Result<ChallengeClock, Error> {
        let (generation, high_water) = match self.read_state()? {
            Some(state) => state,
            None => {
                let generation = random()?;
                self.write_state(generation, 0)?;
                (generation, 0)
            }
        };
        if observed_wall_micros < high_water {
            return Err(Error::ClockReversal);
        }
        let now = self
            .last_now
            .map_or(observed_wall_micros, |last| observed_wall_micros.max(last));
        self.last_now = Some(now);
        if observed_wall_micros > high_water {
            self.write_state(generation, observed_wall_micros)?;
        }
        Ok(ChallengeClock { generation, now })
    }

    fn reclaim(&mut self, now: i64, limit: u32) -> Result<u32, Error> {
        let Some((generation, _)) = self.read_state()? else {
            return Ok(0);
        };
        let rows = self
            .kv
            .range(CHALLENGE_PREFIX, limit as usize)
            .map_err(host)?;
        let mut removed = 0;
        for (key, value) in rows {
            if key.len() != CHALLENGE_PREFIX.len() + ID * 3 {
                return Err(Error::CorruptLog);
            }
            let row_generation = id(&key[CHALLENGE_PREFIX.len()..CHALLENGE_PREFIX.len() + ID])?;
            let record = decode_challenge(
                NodeId(id(
                    &key[CHALLENGE_PREFIX.len() + ID..CHALLENGE_PREFIX.len() + ID * 2]
                )?),
                id(&key[CHALLENGE_PREFIX.len() + ID * 2..])?,
                row_generation,
                &value,
            )?;
            if row_generation != generation || record.used || record.expires_at <= now {
                self.kv.delete(&key).map_err(host)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn challenge(
        &mut self,
        requester: NodeId,
        client_nonce: [u8; 32],
    ) -> Result<Option<ChallengeRecord>, Error> {
        let Some((generation, _)) = self.read_state()? else {
            return Ok(None);
        };
        let Some(value) = self
            .kv
            .get(&challenge_key(&generation, requester, &client_nonce))
            .map_err(host)?
        else {
            return Ok(None);
        };
        Ok(Some(decode_challenge(
            requester,
            client_nonce,
            generation,
            &value,
        )?))
    }

    fn insert_challenge(&mut self, record: &ChallengeRecord) -> Result<(), Error> {
        self.kv
            .put(
                &challenge_key(&record.generation, record.requester, &record.client_nonce),
                encode_challenge(record),
            )
            .map_err(host)
    }

    fn consume_challenge(
        &mut self,
        requester: NodeId,
        client_nonce: [u8; 32],
        now: i64,
    ) -> Result<bool, Error> {
        let Some(mut record) = self.challenge(requester, client_nonce)? else {
            return Ok(false);
        };
        if record.used || record.expires_at <= now {
            return Ok(false);
        }
        record.used = true;
        self.insert_challenge(&record)?;
        Ok(true)
    }

    fn count_challenges(&mut self, requester: Option<NodeId>) -> Result<u64, Error> {
        let Some((generation, _)) = self.read_state()? else {
            return Ok(0);
        };
        let mut prefix = CHALLENGE_PREFIX.to_vec();
        prefix.extend_from_slice(&generation);
        if let Some(requester) = requester {
            prefix.extend_from_slice(&requester.0);
        }
        // Caps are at most 256 rows; the bound keeps counting predictable.
        let rows = self.kv.range(&prefix, 512).map_err(host)?;
        Ok(rows.len() as u64)
    }

    fn take_rate(
        &mut self,
        scope: NodeId,
        now: i64,
        burst: i64,
        refill_per_second: i64,
    ) -> Result<bool, Error> {
        if !(1..=1_000_000).contains(&burst) || !(0..=1_000_000).contains(&refill_per_second) {
            return Err(Error::InvalidRequest);
        }
        let key = rate_key(scope);
        let capacity = burst.saturating_mul(1_000_000);
        let tokens = match self.kv.get(&key).map_err(host)? {
            Some(bytes) if bytes.len() == 16 => {
                let stored = i64::from_be_bytes(bytes[..8].try_into().unwrap());
                let updated = i64::from_be_bytes(bytes[8..].try_into().unwrap());
                stored
                    .saturating_add(
                        refill_per_second.saturating_mul(now.saturating_sub(updated).max(0)),
                    )
                    .min(capacity)
            }
            Some(_) => return Err(Error::CorruptLog),
            None => capacity,
        };
        let cost = 1_000_000;
        let allowed = tokens >= cost;
        let remaining = tokens.saturating_sub(cost);
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&remaining.to_be_bytes());
        bytes.extend_from_slice(&now.to_be_bytes());
        self.kv.put(&key, bytes).map_err(host)?;
        Ok(allowed)
    }

    fn reset_generation(
        &mut self,
        random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
    ) -> Result<[u8; 32], Error> {
        loop {
            let rows = self.kv.range(CHALLENGE_PREFIX, 128).map_err(host)?;
            if rows.is_empty() {
                break;
            }
            for (key, _) in rows {
                self.kv.delete(&key).map_err(host)?;
            }
        }
        let high_water = self.read_state()?.map_or(0, |(_, high_water)| high_water);
        let generation = random()?;
        self.write_state(generation, high_water)?;
        Ok(generation)
    }
}
