//! Versioned, canonical update records. Does not authenticate or accept updates.
mod app;
mod app_view;
pub mod connect;
pub mod https_peer;
pub mod interpreter;
mod json;
pub mod membership;
pub mod metrics;
pub mod websocket;
pub use json::decode_app_json;
mod discovery;
pub mod export;
pub use app_view::{
    ConflictView, ReadView, SaveResult, conflict_view_json, conflict_view_size, read_view_json,
    read_view_size, save_result_json, snapshot_conflict_view_size, snapshot_read_view_size,
};
pub use discovery::{
    ChangeGrantsRequest, ConnectionInfo, FilterSpec, GrantSpec, IssueAppRequest,
    MAX_APP_RESPONSE_BYTES, OAuthRegistration, PeerBinding, PeerRegistration, TLS_NAME, hex, unhex,
    unhex_bytes,
};
pub use harmion_domain as domain;
mod peer;
pub use app::{
    AppId, Command, EditToken, FieldSelection, RequestId, SaveRequest, decode_save_request,
    encode_save_request,
};
pub mod query;
mod record;
pub mod resolution;
pub mod subscription;
pub mod sync_status;
pub use peer::{
    LogEntry, LogEpoch, MAX_PAGE_ENTRIES, MAX_PAGE_RECORD_BYTES, MAX_PEER_BYTES, Page, PeerCommand,
    PeerReply, PeerRequest, PeerResponse, Rejection, decode_peer_request, decode_peer_response,
    encode_peer_request, encode_peer_response,
};
pub use record::{CodecError, MAX_DEPTH, MAX_ITEMS, MAX_RECORD_BYTES, Record, decode, encode};
