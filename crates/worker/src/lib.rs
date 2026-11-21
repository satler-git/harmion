//! Cloudflare Durable Object adapter boundary for the signed-HTTPS peer v2.
//!
//! This crate is the Rust side of a Workers/DO replica: it owns the bounded
//! challenge store over a synchronous durable host transaction, the peer-only
//! node Store over the host SQL boundary, host-only provisioning, the
//! transport-agnostic Peer entry, and a read-only admin API. The wasm-bindgen
//! bridge in `do_bridge` is compiled for wasm32 only. TLS termination, Cloudflare
//! Access, secret distribution, deployment and routing remain host/deployment
//! responsibilities; see docs/implementation-review.md R38-R41.
pub mod admin;
pub mod challenge;
#[cfg(target_arch = "wasm32")]
pub mod do_bridge;
pub mod entry;
pub mod kv;
pub mod outbound;
pub mod sql;
pub mod store;

pub use admin::{bearer_matches, capabilities};
pub use challenge::WorkerChallenges;
pub use entry::{CONTENT_TYPE, HostContext, Request, Response, handle, is_signed_error};
pub use kv::{HostError, KvHost, KvTransaction, MemoryKv};
pub use sql::{SqlHost, SqlTransaction, SqlValue};
pub use store::WorkerStore;
