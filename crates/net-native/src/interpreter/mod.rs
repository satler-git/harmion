//! Provider WebSocket host and invocation broker. Node semantics stay in
//! `harmion_node::interpreter`; this module owns tokio tasks and WS framing.
mod service;
mod ws;

pub use service::Service;
pub(crate) use ws::{switching_protocols, validate_upgrade};
