//! Durable signed-HTTPS challenge state in the node database.
//!
//! This is transport state, not replicated log data: no record or certificate
//! bytes are stored, and consumption is a compare-and-swap, never an inference.
use super::*;
use harmion_node::https_peer::{
    AuthClock, ChallengeClock, ChallengeRecord, ChallengeStore, ChallengeTransaction,
};
use harmion_protocol::https_peer::{Commitment, PayloadClass};

pub(super) fn initialize(sql: &Connection) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE https_state (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            generation BLOB NOT NULL CHECK(length(generation)=32),
            high_water INTEGER NOT NULL);
        CREATE TABLE https_challenges (
            generation BLOB NOT NULL CHECK(length(generation)=32),
            requester BLOB NOT NULL CHECK(length(requester)=32),
            client_nonce BLOB NOT NULL CHECK(length(client_nonce)=32),
            challenge_nonce BLOB NOT NULL CHECK(length(challenge_nonce)=32),
            challenge_request_hash BLOB NOT NULL CHECK(length(challenge_request_hash)=32),
            challenge_hash BLOB NOT NULL CHECK(length(challenge_hash)=32),
            class INTEGER NOT NULL CHECK(class IN (0,1)),
            inner_length INTEGER NOT NULL CHECK(inner_length>=0),
            inner_digest BLOB NOT NULL CHECK(length(inner_digest)=32),
            ttl_ms INTEGER NOT NULL CHECK(ttl_ms>0),
            expires_at INTEGER NOT NULL,
            used INTEGER NOT NULL CHECK(used IN (0,1)),
            PRIMARY KEY(generation,requester,client_nonce));
        CREATE TABLE https_rate (
            scope BLOB PRIMARY KEY CHECK(length(scope)=32),
            tokens INTEGER NOT NULL CHECK(tokens>=0),
            updated_at INTEGER NOT NULL);
        PRAGMA user_version=12;",
    )
    .map_err(db)
}

impl ChallengeStore for SqliteStore {
    fn challenge_transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn ChallengeTransaction) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let auth_clock = &mut self.auth_clock;
        let result = {
            let mut state = ChallengeState {
                sql: &tx,
                auth_clock,
            };
            f(&mut state)
        };
        let value = result?;
        tx.commit().map_err(db)?;
        Ok(value)
    }
}

struct ChallengeState<'a> {
    sql: &'a rusqlite::Transaction<'a>,
    auth_clock: &'a mut AuthClock,
}

impl ChallengeState<'_> {
    fn generation(&self) -> Result<Option<[u8; 32]>, Error> {
        let generation: Option<Vec<u8>> = self
            .sql
            .query_row(
                "SELECT generation FROM https_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;
        generation
            .map(|bytes| bytes.try_into().map_err(|_| Error::CorruptLog))
            .transpose()
    }
}

impl ChallengeTransaction for ChallengeState<'_> {
    fn clock(
        &mut self,
        observed_wall_micros: i64,
        random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
    ) -> Result<ChallengeClock, Error> {
        let row: Option<(Vec<u8>, i64)> = self
            .sql
            .query_row(
                "SELECT generation,high_water FROM https_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        let (generation, high_water) = match row {
            Some((generation, high_water)) => (
                <[u8; 32]>::try_from(generation.as_slice()).map_err(|_| Error::CorruptLog)?,
                high_water,
            ),
            None => {
                let generation = random()?;
                self.sql
                    .execute("INSERT INTO https_state VALUES(1,?1,0)", [generation])
                    .map_err(db)?;
                (generation, 0)
            }
        };
        let now = self.auth_clock.adjust(observed_wall_micros, high_water)?;
        // Only the observed wall time advances the durable high-water; the
        // monotonic clamp above cannot force it past the caller's clock.
        self.sql
            .execute(
                "UPDATE https_state SET high_water=?1 WHERE singleton=1",
                [observed_wall_micros],
            )
            .map_err(db)?;
        Ok(ChallengeClock { generation, now })
    }

    fn reclaim(&mut self, now: i64, limit: u32) -> Result<u32, Error> {
        let current = self.generation()?;
        let removed = self
            .sql
            .execute(
                "DELETE FROM https_challenges WHERE rowid IN (
                    SELECT rowid FROM https_challenges
                    WHERE used=1 OR expires_at<=?1 OR generation<>?2
                    LIMIT ?3)",
                params![now, current.unwrap_or([0; 32]), limit],
            )
            .map_err(db)?;
        removed.try_into().map_err(|_| Error::CorruptLog)
    }

    fn challenge(
        &mut self,
        requester: NodeId,
        client_nonce: [u8; 32],
    ) -> Result<Option<ChallengeRecord>, Error> {
        let Some(generation) = self.generation()? else {
            return Ok(None);
        };
        let row = self
            .sql
            .query_row(
                "SELECT challenge_nonce,challenge_request_hash,challenge_hash,class,inner_length,
                    inner_digest,ttl_ms,expires_at,used
                 FROM https_challenges
                 WHERE generation=?1 AND requester=?2 AND client_nonce=?3",
                params![generation, requester.0, client_nonce],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, bool>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(
            |(
                challenge_nonce,
                challenge_request_hash,
                challenge_hash,
                class,
                inner_length,
                inner_digest,
                ttl_ms,
                expires_at,
                used,
            )| {
                Ok(ChallengeRecord {
                    requester,
                    client_nonce,
                    generation,
                    challenge_nonce: fixed(challenge_nonce)?,
                    challenge_request_hash: fixed(challenge_request_hash)?,
                    challenge_hash: fixed(challenge_hash)?,
                    commitment: Commitment {
                        class: match class {
                            0 => PayloadClass::Data,
                            1 => PayloadClass::Evidence,
                            _ => return Err(Error::CorruptLog),
                        },
                        inner_length: inner_length.try_into().map_err(|_| Error::CorruptLog)?,
                        inner_digest: fixed(inner_digest)?,
                    },
                    ttl_ms: ttl_ms.try_into().map_err(|_| Error::CorruptLog)?,
                    expires_at,
                    used,
                })
            },
        )
        .transpose()
    }

    fn insert_challenge(&mut self, record: &ChallengeRecord) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO https_challenges VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    record.generation,
                    record.requester.0,
                    record.client_nonce,
                    record.challenge_nonce,
                    record.challenge_request_hash,
                    record.challenge_hash,
                    record.commitment.class as i64,
                    i64::from(record.commitment.inner_length),
                    record.commitment.inner_digest,
                    i64::from(record.ttl_ms),
                    record.expires_at,
                    record.used,
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn consume_challenge(
        &mut self,
        requester: NodeId,
        client_nonce: [u8; 32],
        now: i64,
    ) -> Result<bool, Error> {
        let Some(generation) = self.generation()? else {
            return Ok(false);
        };
        let changed = self
            .sql
            .execute(
                "UPDATE https_challenges SET used=1
                 WHERE generation=?1 AND requester=?2 AND client_nonce=?3
                   AND used=0 AND expires_at>?4",
                params![generation, requester.0, client_nonce, now],
            )
            .map_err(db)?;
        Ok(changed == 1)
    }

    fn count_challenges(&mut self, requester: Option<NodeId>) -> Result<u64, Error> {
        let Some(generation) = self.generation()? else {
            return Ok(0);
        };
        let count: i64 = match requester {
            Some(requester) => self
                .sql
                .query_row(
                    "SELECT COUNT(*) FROM https_challenges WHERE generation=?1 AND requester=?2",
                    params![generation, requester.0],
                    |row| row.get(0),
                )
                .map_err(db)?,
            None => self
                .sql
                .query_row(
                    "SELECT COUNT(*) FROM https_challenges WHERE generation=?1",
                    [generation],
                    |row| row.get(0),
                )
                .map_err(db)?,
        };
        count.try_into().map_err(|_| Error::CorruptLog)
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
        let capacity = burst.saturating_mul(1_000_000);
        let row: Option<(i64, i64)> = self
            .sql
            .query_row(
                "SELECT tokens,updated_at FROM https_rate WHERE scope=?1",
                [scope.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        let tokens = match row {
            None => capacity,
            Some((tokens, updated_at)) => tokens
                .saturating_add(
                    refill_per_second.saturating_mul(now.saturating_sub(updated_at).max(0)),
                )
                .min(capacity),
        };
        let cost = 1_000_000;
        let remaining = tokens.saturating_sub(cost);
        if remaining < 0 {
            self.sql
                .execute(
                    "INSERT INTO https_rate VALUES(?1,?2,?3)
                     ON CONFLICT(scope) DO UPDATE SET tokens=excluded.tokens,updated_at=excluded.updated_at",
                    params![scope.0, tokens, now],
                )
                .map_err(db)?;
            return Ok(false);
        }
        self.sql
            .execute(
                "INSERT INTO https_rate VALUES(?1,?2,?3)
                 ON CONFLICT(scope) DO UPDATE SET tokens=excluded.tokens,updated_at=excluded.updated_at",
                params![scope.0, remaining, now],
            )
            .map_err(db)?;
        Ok(true)
    }

    fn reset_generation(
        &mut self,
        random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
    ) -> Result<[u8; 32], Error> {
        self.sql
            .execute("DELETE FROM https_challenges", [])
            .map_err(db)?;
        let generation = random()?;
        self.sql
            .execute(
                "INSERT INTO https_state VALUES(1,?1,0)
                 ON CONFLICT(singleton) DO UPDATE SET generation=excluded.generation",
                [generation],
            )
            .map_err(db)?;
        Ok(generation)
    }
}
