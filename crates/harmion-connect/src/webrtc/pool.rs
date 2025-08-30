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
mod tests {
    // Testing framework: Rust built-in #[test] + #[tokio::test] for async
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn default_initializes_empty_map_and_cache() {
        let pool = PeerPool::default();
        assert!(pool.map.is_empty(), "Peer map should start empty");

        let origin: [u8; 32] = [1u8; 32];
        let nonce: [u8; NONCE_LEN] = [2u8; NONCE_LEN];

        pool.replay_cache.insert((origin, nonce), nonce).await;
        let got = pool.replay_cache.get(&(origin, nonce)).await;

        assert_eq!(got, Some(nonce), "Cache insert/get should round-trip");
    }

    #[tokio::test]
    async fn replay_cache_distinguishes_origin_and_nonce() {
        let pool = PeerPool::default();

        let origin_a: [u8; 32] = [3u8; 32];
        let origin_b: [u8; 32] = [4u8; 32];
        let nonce_x: [u8; NONCE_LEN] = [5u8; NONCE_LEN];
        let nonce_y: [u8; NONCE_LEN] = [6u8; NONCE_LEN];

        pool.replay_cache.insert((origin_a, nonce_x), nonce_x).await;

        assert!(pool.replay_cache.get(&(origin_a, nonce_y)).await.is_none(), "Different nonce should miss");
        assert!(pool.replay_cache.get(&(origin_b, nonce_x)).await.is_none(), "Different origin should miss");
        assert_eq!(pool.replay_cache.get(&(origin_a, nonce_x)).await, Some(nonce_x), "Exact key should hit");
    }

    #[tokio::test]
    async fn concurrent_cache_put_get() {
        let pool = Arc::new(PeerPool::default());
        let mut tasks = Vec::new();

        for i in 0..8u8 {
            let pool_cloned = Arc::clone(&pool);
            tasks.push(tokio::spawn(async move {
                let origin = [i; 32];
                let nonce = [i.wrapping_add(1); NONCE_LEN];
                pool_cloned.replay_cache.insert((origin, nonce), nonce).await;
                let got = pool_cloned.replay_cache.get(&(origin, nonce)).await;
                assert_eq!(got, Some(nonce));
            }));
        }

        for t in tasks {
            t.await.expect("task join should succeed");
        }
    }

    #[tokio::test]
    async fn multiple_nonces_same_origin_are_independent() {
        let pool = PeerPool::default();
        let origin = [42u8; 32];

        for i in 0..5u8 {
            let nonce = [i; NONCE_LEN];
            pool.replay_cache.insert((origin, nonce), nonce).await;
        }

        for i in 0..5u8 {
            let nonce = [i; NONCE_LEN];
            assert_eq!(pool.replay_cache.get(&(origin, nonce)).await, Some(nonce));
        }

        let missing = [99u8; NONCE_LEN];
        assert!(pool.replay_cache.get(&(origin, missing)).await.is_none());
    }

    #[test]
    fn constants_are_expected() {
        assert_eq!(TTL_SECS, 10 * 60, "TTL should be 10 minutes");
        assert_eq!(MAX_CAPACITY, 10_000, "Capacity should be 10k");
    }

    #[test]
    fn peer_pool_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PeerPool>();
    }
}