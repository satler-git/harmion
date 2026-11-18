//! Versioned, canonical update records. Does not authenticate or accept updates.
mod app;
mod app_view;
mod json;
pub mod membership;
pub use json::decode_app_json;
mod discovery;
pub use app_view::{
    ConflictView, ReadView, SaveResult, conflict_view_json, read_view_json, save_result_json,
};
pub use discovery::{
    ChangeGrantsRequest, ConnectionInfo, FilterSpec, GrantSpec, IssueAppRequest,
    MAX_APP_RESPONSE_BYTES, PeerBinding, PeerRegistration, TLS_NAME, hex, unhex, unhex_bytes,
};
pub use harmion_domain as domain;
mod peer;
pub use app::{
    AppId, Command, EditToken, RequestId, SaveRequest, decode_save_request, encode_save_request,
};
mod record;
pub use peer::{
    LogEntry, LogEpoch, MAX_PAGE_ENTRIES, MAX_PAGE_RECORD_BYTES, MAX_PEER_BYTES, Page, PeerCommand,
    PeerReply, PeerRequest, PeerResponse, Rejection, decode_peer_request, decode_peer_response,
    encode_peer_request, encode_peer_response,
};
pub use record::{CodecError, MAX_DEPTH, MAX_ITEMS, MAX_RECORD_BYTES, Record, decode, encode};
