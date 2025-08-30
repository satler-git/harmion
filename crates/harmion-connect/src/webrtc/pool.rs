use dashmap::DashMap;
use moka::future::Cache;

use std::time::Duration;

use crate::{
    webrtc::simple::{Connected, Peer},
    PeerIndex, NONCE_LEN,
};

// 機能
//
// なるべくblockしないのが目標
//
// dashmap<id(pubkey), peer>
//
// add(&self, peer) -> Result<()>
//  -> 相手と初期の情報を交換する
//  - pubkey
//  - (if sig) siginfo
//
// disconnect(&self, peer) -> Result<()>
//
// send<T>(&self, &peerid, T)
// recv(&self, &peerid) -> Result<Message>
//  -> 必ずpeeridからMessage一つで返す。verifyに失敗しても(Err)
//
// verify
// timestamp(10m(const)) -> verify sign -> replay-cache(10m(const))

pub(crate) struct PeerPool {
    // pubkey, peer
    map: DashMap<[u8; 32], Peer<Connected>>,
    // origin, nonce
    replay_cache: Cache<([u8; 32], [u8; NONCE_LEN]), [u8; NONCE_LEN]>,
}

const TTL_SECS: u64 = 10 * 60;
const MAX_CAPACITY: u64 = 10_000;

impl Default for PeerPool {
    fn default() -> Self {
        PeerPool {
            map: DashMap::new(),
            replay_cache: Cache::builder()
                .max_capacity(MAX_CAPACITY)
                .time_to_live(Duration::from_secs(TTL_SECS))
                .build(),
        }
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum PoolError {}

type PoolResult<T> = Result<T, PoolError>;

impl PeerPool {
    pub fn add(&self, _peer: Peer<Connected>, _alias: Option<&str>) -> PoolResult<PeerIndex> {
        todo!()
    }
}

#[cfg(test)]
mod tests_pool {
    // Testing stack:
    // - Rust built-in test harness
    // - Tokio runtime via #[tokio::test] (Tokio is already a dependency in this workspace)
    // Focus: PeerPool defaults, replay_cache semantics, and thread-safety guarantees.

    use super::*;

    #[test]
    fn constants_are_expected() {
        assert_eq!(TTL_SECS, 10 * 60, "TTL_SECS should remain a 10-minute TTL");
        assert_eq!(MAX_CAPACITY, 10_000, "MAX_CAPACITY should be 10_000");
    }

    #[test]
    fn default_state_is_empty_and_ready() {
        let pool = PeerPool::default();
        assert_eq!(pool.map.len(), 0, "Peer map should start empty");
        assert_eq!(pool.replay_cache.entry_count(), 0, "Replay cache should start empty");
    }

    #[tokio::test]
    async fn replay_cache_get_after_insert() {
        let pool = PeerPool::default();
        let origin: [u8; 32] = [1u8; 32];
        let nonce: [u8; NONCE_LEN] = [42u8; NONCE_LEN];

        pool.replay_cache.insert((origin, nonce), nonce).await;
        let got = pool.replay_cache.get(&(origin, nonce)).await;

        assert_eq!(got, Some(nonce), "Cache get should return the inserted value");
    }

    #[tokio::test]
    async fn replay_cache_key_isolation_by_origin() {
        let pool = PeerPool::default();
        let origin_a: [u8; 32] = [2u8; 32];
        let origin_b: [u8; 32] = [3u8; 32];
        let nonce: [u8; NONCE_LEN] = [9u8; NONCE_LEN];

        pool.replay_cache.insert((origin_a, nonce), nonce).await;

        let miss_other_origin = pool.replay_cache.get(&(origin_b, nonce)).await;
        assert!(miss_other_origin.is_none(), "Different origin with same nonce must miss");
    }

    #[tokio::test]
    async fn replay_cache_key_isolation_by_nonce() {
        let pool = PeerPool::default();
        let origin: [u8; 32] = [4u8; 32];
        let nonce_a: [u8; NONCE_LEN] = [7u8; NONCE_LEN];
        let nonce_b: [u8; NONCE_LEN] = [8u8; NONCE_LEN];

        pool.replay_cache.insert((origin, nonce_a), nonce_a).await;

        let miss_other_nonce = pool.replay_cache.get(&(origin, nonce_b)).await;
        assert!(miss_other_nonce.is_none(), "Same origin with different nonce must miss");
    }

    #[tokio::test]
    async fn replay_cache_overwrite_same_key() {
        let pool = PeerPool::default();
        let origin: [u8; 32] = [5u8; 32];
        let nonce: [u8; NONCE_LEN] = [11u8; NONCE_LEN];
        let value1: [u8; NONCE_LEN] = [1u8; NONCE_LEN];
        let value2: [u8; NONCE_LEN] = [2u8; NONCE_LEN];

        pool.replay_cache.insert((origin, nonce), value1).await;
        pool.replay_cache.insert((origin, nonce), value2).await;

        let got = pool.replay_cache.get(&(origin, nonce)).await;
        assert_eq!(got, Some(value2), "Second insert should overwrite the value for the same key");
    }

    #[tokio::test]
    async fn replay_cache_contains_and_invalidate() {
        let pool = PeerPool::default();
        let origin: [u8; 32] = [6u8; 32];
        let nonce: [u8; NONCE_LEN] = [3u8; NONCE_LEN];

        pool.replay_cache.insert((origin, nonce), nonce).await;
        assert!(pool.replay_cache.contains_key(&(origin, nonce)), "contains_key should be true after insert");

        // Invalidate should remove the entry
        pool.replay_cache.invalidate(&(origin, nonce));
        let got = pool.replay_cache.get(&(origin, nonce)).await;
        assert!(got.is_none(), "Entry should be gone after invalidate");
    }

    #[tokio::test]
    async fn capacity_sanity_small_batch() {
        let pool = PeerPool::default();

        // Insert a modest number of distinct entries to avoid heavy tests.
        for i in 0u16..128 {
            let mut origin = [0u8; 32];
            origin[0] = (i & 0xFF) as u8;
            origin[1] = ((i >> 8) & 0xFF) as u8;
            let nonce: [u8; NONCE_LEN] = [i as u8; NONCE_LEN];
            pool.replay_cache.insert((origin, nonce), nonce).await;
        }

        let count = pool.replay_cache.entry_count();
        assert!(count >= 100, "Entry count should reflect many inserts (got {})", count);
    }

    #[test]
    fn peer_pool_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PeerPool>();
    }

    // Placeholder: PeerPool::add is currently `todo!()`. Keep a reminder test ignored until implemented.
    #[test]
    #[ignore = "PeerPool::add is not implemented yet; enable this when the method is ready"]
    fn add_inserts_peer_and_returns_index_placeholder() {
        let _pool = PeerPool::default();
        assert!(true);
    }
}