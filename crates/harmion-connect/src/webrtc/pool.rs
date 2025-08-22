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
    pub fn add(&self, peer: Peer<Connected>, alias: Option<&str>) -> PoolResult<PeerIndex> {
        todo!()
    }
}
