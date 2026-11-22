//! Worker-initiated (outbound) peer synchronization.
//!
//! The same node core and frame codec run in the Worker; only the transport is
//! different: a `fetch`-based SignedHttps client instead of native TCP/TLS. The
//! sync round mirrors the native algorithm: membership evidence first, then
//! Status/Cursor/Page/Accept with durable cursors. Store transactions are never
//! held across an await.
use harmion_node::{
    Error,
    membership::{self, Peer},
    membership_log, replication,
    storage::{Identity, Store},
};
use harmion_protocol::{
    Page, PeerCommand, PeerReply,
    membership::{Action, Binding, Certificate},
};

#[derive(Debug)]
pub enum OutboundError {
    Node(Error),
    Transport(String),
    Invalid(String),
}
impl std::fmt::Display for OutboundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Node(error) => write!(f, "node: {error}"),
            Self::Transport(message) => write!(f, "transport: {message}"),
            Self::Invalid(message) => write!(f, "invalid: {message}"),
        }
    }
}
impl From<Error> for OutboundError {
    fn from(error: Error) -> Self {
        Self::Node(error)
    }
}

#[derive(Default)]
pub struct Report {
    pub uploaded: u64,
    pub downloaded: u64,
    pub membership_uploaded: u64,
    pub membership_downloaded: u64,
    pub local_high: u64,
    pub remote_high: u64,
}

/// One signed exchange with a peer. Implementations must verify the challenge
/// proof, sign only the exact live attempt, and verify the bound response.
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, OutboundError>;
}

pub async fn sync<S: Store, T: Transport>(
    store: &mut S,
    transport: &T,
    local: Identity,
    peer: Peer,
) -> Result<Report, OutboundError> {
    if peer.node == local.node {
        return Err(OutboundError::Invalid("cannot sync with self".into()));
    }

    // Membership evidence: pull newer certificates, push local ones.
    let local_membership_high = store.transaction(membership_log::high)?;
    let PeerReply::Status {
        epoch: remote_epoch,
        high: remote_membership_high,
    } = transport
        .exchange(local, &peer, PeerCommand::MembershipStatus)
        .await?
    else {
        return Err(OutboundError::Invalid("membership status".into()));
    };
    let PeerReply::Cursor {
        epoch: echoed,
        through: mut upload,
    } = transport
        .exchange(
            local,
            &peer,
            PeerCommand::MembershipCursor { epoch: local.epoch },
        )
        .await?
    else {
        return Err(OutboundError::Invalid("membership cursor".into()));
    };
    if echoed != local.epoch || upload > local_membership_high {
        return Err(OutboundError::Invalid("membership upload cursor".into()));
    }
    let mut download = store.transaction(|tx| tx.membership_cursor(peer.node, remote_epoch))?;
    if download > remote_membership_high {
        match transport
            .exchange(local, &peer, PeerCommand::MembershipStatus)
            .await?
        {
            PeerReply::Status { epoch, high } if epoch == remote_epoch && download <= high => {}
            _ => return Err(OutboundError::Invalid("membership recheck".into())),
        }
    }
    let mut report = Report::default();
    while upload < local_membership_high || download < remote_membership_high {
        if upload < local_membership_high {
            let page = store.transaction(|tx| {
                membership_log::page(tx, local.epoch, upload, local_membership_high)
            })?;
            let through = page.through;
            let count = page.entries.len().max(1) as u64;
            match transport
                .exchange(local, &peer, PeerCommand::MembershipAccept(page))
                .await?
            {
                PeerReply::Ack {
                    epoch,
                    through: ack,
                } if epoch == local.epoch && ack == through && ack > upload => {
                    upload = ack;
                    report.membership_uploaded += count;
                }
                PeerReply::CursorMismatch { epoch, current }
                    if epoch == local.epoch && current > upload =>
                {
                    if current > local_membership_high {
                        return Err(OutboundError::Invalid("membership ack cursor".into()));
                    }
                    upload = current;
                }
                _ => return Err(OutboundError::Invalid("membership accept".into())),
            }
        }
        if download < remote_membership_high {
            let PeerReply::MembershipPage(page) = transport
                .exchange(
                    local,
                    &peer,
                    PeerCommand::MembershipPage {
                        epoch: remote_epoch,
                        after: download,
                        high: remote_membership_high,
                    },
                )
                .await?
            else {
                return Err(OutboundError::Invalid("membership page".into()));
            };
            if page.epoch != remote_epoch
                || page.after != download
                || page.high != remote_membership_high
                || page.through <= download
            {
                return Err(OutboundError::Invalid("membership page shape".into()));
            }
            validate_membership_page(&page)?;
            let count = page.entries.len().max(1) as u64;
            let through = store.transaction(|tx| membership_log::receive(tx, peer.node, &page))?;
            download = through;
            report.membership_downloaded += count;
        }
    }

    // Records: same Status/Cursor/Page/Accept shape as native sync.
    let local_high = store.transaction(replication::high)?;
    let PeerReply::Status {
        epoch,
        high: remote_high,
    } = transport
        .exchange(local, &peer, PeerCommand::Status)
        .await?
    else {
        return Err(OutboundError::Invalid("status".into()));
    };
    let PeerReply::Cursor {
        epoch: echoed,
        through: mut up,
    } = transport
        .exchange(local, &peer, PeerCommand::Cursor { epoch: local.epoch })
        .await?
    else {
        return Err(OutboundError::Invalid("cursor".into()));
    };
    if echoed != local.epoch || up > local_high {
        return Err(OutboundError::Invalid("upload cursor".into()));
    }
    let mut down = store.transaction(|tx| {
        membership::active_peer(tx, peer.node)?;
        tx.cursor(peer.node, epoch)
    })?;
    if down > remote_high {
        match transport
            .exchange(local, &peer, PeerCommand::Status)
            .await?
        {
            PeerReply::Status {
                epoch: current,
                high,
            } if current == epoch && down <= high => {}
            _ => return Err(OutboundError::Invalid("recheck".into())),
        }
    }
    report.local_high = local_high;
    report.remote_high = remote_high;
    while up < local_high || down < remote_high {
        if up < local_high {
            let page = store.transaction(|tx| {
                membership::active_peer(tx, peer.node)?;
                replication::page(tx, local.epoch, up, local_high)
            })?;
            let through = page.through;
            let count = page.entries.len() as u64;
            match transport
                .exchange(local, &peer, PeerCommand::Accept(page))
                .await?
            {
                PeerReply::Ack {
                    epoch: received,
                    through: accepted,
                } if received == local.epoch && accepted == through && accepted > up => {
                    up = accepted;
                    report.uploaded += count;
                }
                PeerReply::CursorMismatch {
                    epoch: received,
                    current,
                } if received == local.epoch && current > up => {
                    if current > local_high {
                        return Err(OutboundError::Invalid("accept cursor".into()));
                    }
                    up = current;
                }
                _ => return Err(OutboundError::Invalid("accept".into())),
            }
        }
        if down < remote_high {
            let PeerReply::Page(page) = transport
                .exchange(
                    local,
                    &peer,
                    PeerCommand::Page {
                        epoch,
                        after: down,
                        high: remote_high,
                    },
                )
                .await?
            else {
                return Err(OutboundError::Invalid("page".into()));
            };
            if page.epoch != epoch
                || page.after != down
                || page.high != remote_high
                || page.through <= down
            {
                return Err(OutboundError::Invalid("page shape".into()));
            }
            let count = page.entries.len() as u64;
            let expected = peer.clone();
            let through = store.transaction(|tx| {
                let active = membership::active_peer(tx, expected.node)?;
                if active.key != expected.key || active.endpoint != expected.endpoint {
                    return Err(Error::IdentityMismatch);
                }
                replication::receive(tx, expected.node, &page)
            })?;
            down = through;
            report.downloaded += count;
        }
    }
    Ok(report)
}

fn validate_membership_page(page: &Page<Certificate>) -> Result<(), OutboundError> {
    replication::validate_page(page, Certificate::bytes)?;
    for entry in &page.entries {
        let Action::Admit(member) = &entry.record.statement().action else {
            return Err(OutboundError::Invalid("membership action".into()));
        };
        harmion_node::identity::validate_signing_key(&member.signing_key)?;
        if let Binding::SignedHttps(key) = &member.binding {
            if *key != member.signing_key {
                return Err(OutboundError::Invalid("binding key".into()));
            }
            harmion_protocol::https_peer::validate_origin(&member.endpoint)
                .map_err(|error| OutboundError::Node(Error::Codec(error)))?;
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
mod fetch_transport {
    use super::{OutboundError, Transport};
    use crate::do_bridge::random_bytes32;
    use harmion_node::Error;
    use harmion_node::https_peer::verify_frame;
    use harmion_node::membership::{Peer, PeerKey};
    use harmion_node::membership_state::MembershipSigner;
    use harmion_node::storage::{Identity, Store};
    use harmion_protocol::{
        PeerCommand, PeerReply, PeerRequest, decode_peer_response, encode_peer_request,
        https_peer::{
            Body, CHALLENGE_PATH, Commitment, Common, EXCHANGE_PATH, ErrorCode, ExchangeContext,
            Frame, MAX_FRAME_BYTES, PayloadClass, ResponseResult,
        },
    };
    use js_sys::{Promise, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue, prelude::*};
    use wasm_bindgen_futures::JsFuture;

    #[wasm_bindgen(module = "host-api")]
    extern "C" {
        #[wasm_bindgen(js_name = fetchPeer, catch)]
        fn fetch_peer(url: &str, body: &[u8]) -> Result<Promise, JsValue>;
    }

    pub struct FetchTransport {
        pub signer: MembershipSigner,
        pub allow_internal: bool,
    }

    fn codec(error: harmion_protocol::CodecError) -> OutboundError {
        OutboundError::Node(Error::Codec(error))
    }

    fn invalid(message: impl Into<String>) -> OutboundError {
        OutboundError::Invalid(message.into())
    }

    fn validate_origin(peer: &Peer, allow_internal: bool) -> Result<(), OutboundError> {
        let url =
            url::Url::parse(&peer.endpoint).map_err(|_| invalid("peer origin is not a URL"))?;
        match url.scheme() {
            "https" => Ok(()),
            "http"
                if allow_internal
                    && url
                        .host_str()
                        .is_some_and(|host| host.ends_with(".internal")) =>
            {
                Ok(())
            }
            _ => Err(invalid("peer origin must be https")),
        }
    }

    async fn fetch(url: &str, body: &[u8]) -> Result<(u16, Vec<u8>), OutboundError> {
        let promise = fetch_peer(url, body).map_err(|error| {
            OutboundError::Transport(error.as_string().unwrap_or_else(|| "fetch".into()))
        })?;
        let value = JsFuture::from(promise)
            .await
            .map_err(|error| OutboundError::Transport(format!("{error:?}")))?;
        let status = Reflect::get(&value, &JsValue::from_str("status"))
            .ok()
            .and_then(|value| value.as_f64())
            .ok_or_else(|| invalid("fetch status"))? as u16;
        let body = Reflect::get(&value, &JsValue::from_str("body"))
            .ok()
            .and_then(|value| value.dyn_into::<Uint8Array>().ok())
            .ok_or_else(|| invalid("fetch body"))?;
        Ok((status, body.to_vec()))
    }

    impl Transport for FetchTransport {
        async fn exchange(
            &self,
            local: Identity,
            peer: &Peer,
            command: PeerCommand,
        ) -> Result<PeerReply, OutboundError> {
            let PeerKey::SignedHttps(server_key) = &peer.key else {
                return Err(invalid("peer is not signed-HTTPS"));
            };
            let server_key = *server_key;
            validate_origin(peer, self.allow_internal)?;
            if peer.node == local.node {
                return Err(invalid("self peer"));
            }
            let origin = peer.endpoint.trim_end_matches('/').to_owned();
            let request = encode_peer_request(&PeerRequest {
                cluster: local.cluster,
                node: local.node,
                command: command.clone(),
            })
            .map_err(codec)?;
            let commitment = Commitment::from_request(&request).map_err(codec)?;
            let nonce = random_bytes32().map_err(|_| OutboundError::Transport("entropy".into()))?;
            let common = Common {
                cluster: local.cluster,
                sender: local.node,
                audience: peer.node,
                origin: origin.clone(),
            };
            let challenge_request = self
                .signer
                .sign_challenge_request(common.clone(), nonce, commitment.clone())
                .map_err(OutboundError::Node)?;
            let (status, body) = fetch(
                &format!("{origin}{CHALLENGE_PATH}"),
                &challenge_request.encode().map_err(codec)?,
            )
            .await?;
            if status != 200 {
                if let Ok(frame) = Frame::decode(&body)
                    && verify_frame(&frame, &server_key).is_ok()
                    && let Body::ChallengeError { error, .. } = &frame.body
                    && error.code == ErrorCode::Capacity
                {
                    return Err(OutboundError::Transport("peer capacity".into()));
                }
                return Err(invalid("challenge status"));
            }
            let challenge = Frame::decode(&body).map_err(codec)?;
            verify_frame(&challenge, &server_key)
                .map_err(|_| OutboundError::Node(Error::InvalidSignature))?;
            let expected = Common {
                cluster: local.cluster,
                sender: peer.node,
                audience: local.node,
                origin: origin.clone(),
            };
            if challenge.common != expected {
                return Err(invalid("challenge common"));
            }
            let Body::Challenge {
                challenge_request_hash,
                client_nonce,
                generation,
                challenge_nonce,
                ttl_ms,
                commitment: echoed,
            } = &challenge.body
            else {
                return Err(invalid("challenge body"));
            };
            if *ttl_ms == 0
                || *challenge_request_hash != challenge_request.hash().map_err(codec)?
                || *client_nonce != nonce
                || *echoed != commitment
            {
                return Err(invalid("challenge binding"));
            }
            let context = ExchangeContext {
                challenge_request_hash: *challenge_request_hash,
                challenge_hash: challenge.hash().map_err(codec)?,
                client_nonce: nonce,
                generation: *generation,
                challenge_nonce: *challenge_nonce,
            };
            let class = match &command {
                PeerCommand::MembershipStatus
                | PeerCommand::MembershipCursor { .. }
                | PeerCommand::MembershipPage { .. }
                | PeerCommand::MembershipAccept(_) => PayloadClass::Evidence,
                _ => PayloadClass::Data,
            };
            let exchange = self
                .signer
                .sign_exchange(common, context.clone(), class, request)
                .map_err(OutboundError::Node)?;
            let exchange_hash = exchange.hash().map_err(codec)?;
            let (status, body) = fetch(
                &format!("{origin}{EXCHANGE_PATH}"),
                &exchange.encode().map_err(codec)?,
            )
            .await?;
            if body.len() > MAX_FRAME_BYTES {
                return Err(invalid("response too large"));
            }
            let frame = Frame::decode(&body).map_err(codec)?;
            verify_frame(&frame, &server_key)
                .map_err(|_| OutboundError::Node(Error::InvalidSignature))?;
            let Body::Response {
                context: bound,
                result,
            } = &frame.body
            else {
                return Err(invalid("response body"));
            };
            let expected_status = match result {
                ResponseResult::Peer(_) => 200,
                ResponseResult::Error(error) => error.code.http_status(),
            };
            if bound.exchange_hash != exchange_hash
                || bound.challenge_hash != context.challenge_hash
                || bound.client_nonce != nonce
                || bound.generation != context.generation
                || bound.challenge_nonce != context.challenge_nonce
                || status != expected_status
            {
                return Err(invalid("response binding"));
            }
            match result {
                ResponseResult::Peer(bytes) => {
                    let reply = decode_peer_response(bytes).map_err(codec)?;
                    if reply.cluster != local.cluster || reply.node != peer.node {
                        return Err(invalid("reply identity"));
                    }
                    if let PeerReply::Rejected { reason, .. } = reply.reply {
                        return Err(OutboundError::Invalid(format!("rejected: {reason:?}")));
                    }
                    Ok(reply.reply)
                }
                ResponseResult::Error(error) => Err(match error.code {
                    ErrorCode::NotAuthorized => OutboundError::Node(Error::Unauthenticated),
                    _ => OutboundError::Transport(format!("signed error {:?}", error.code)),
                }),
            }
        }
    }

    #[wasm_bindgen]
    pub fn outbound_peers(
        cluster: String,
        node: String,
        epoch: String,
    ) -> Result<JsValue, JsValue> {
        let (mut store, _) = crate::do_bridge::open(&cluster, &node, &epoch)?;
        let peers = store
            .transaction(|tx| {
                let mut result = Vec::new();
                for peer in tx.peers()? {
                    if peer.retired {
                        continue;
                    }
                    if !matches!(peer.key, PeerKey::SignedHttps(_)) {
                        continue;
                    }
                    if harmion_node::membership::active_peer(tx, peer.node).is_err() {
                        continue;
                    }
                    result.push(harmion_protocol::hex(&peer.node.0));
                }
                Ok(result)
            })
            .map_err(|error| JsValue::from_str(&format!("peers: {error}")))?;
        Ok(
            js_sys::Array::from_iter(peers.into_iter().map(|value| JsValue::from_str(&value)))
                .into(),
        )
    }

    #[wasm_bindgen]
    pub async fn outbound_sync_list(
        cluster: String,
        node: String,
        epoch: String,
        seed: String,
        allow_internal: bool,
        peers: String,
    ) -> Result<JsValue, JsValue> {
        let (mut store, local) = crate::do_bridge::open(&cluster, &node, &epoch)?;
        let signer = MembershipSigner::from_bytes(
            &harmion_protocol::unhex::<32>(&seed)
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
        );
        let transport = FetchTransport {
            signer,
            allow_internal,
        };
        let mut results = Vec::new();
        for peer_hex in peers.split(',').filter(|value| !value.is_empty()) {
            let peer_id = match harmion_protocol::unhex::<32>(peer_hex) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let target = match store.transaction(|tx| tx.peer(harmion_domain::NodeId(peer_id))) {
                Ok(Some(peer)) => peer,
                _ => continue,
            };
            let value = match super::sync(&mut store, &transport, local, target).await {
                Ok(report) => serde_json::json!({
                    "peer": peer_hex,
                    "ok": true,
                    "uploaded": report.uploaded,
                    "downloaded": report.downloaded,
                }),
                Err(error) => serde_json::json!({
                    "peer": peer_hex,
                    "ok": false,
                    "error": error.to_string(),
                }),
            };
            results.push(value);
        }
        let value = serde_json::Value::Array(results);
        js_sys::JSON::parse(&value.to_string())
    }

    #[wasm_bindgen]
    pub async fn outbound_sync(
        cluster: String,
        node: String,
        epoch: String,
        seed: String,
        peer: String,
        allow_internal: bool,
    ) -> Result<JsValue, JsValue> {
        let (mut store, local) = crate::do_bridge::open(&cluster, &node, &epoch)?;
        let signer = MembershipSigner::from_bytes(
            &harmion_protocol::unhex::<32>(&seed).map_err(|e| JsValue::from_str(&e.to_string()))?,
        );
        let peer_id = harmion_protocol::unhex::<32>(&peer)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let target: Peer = store
            .transaction(|tx| tx.peer(harmion_domain::NodeId(peer_id)))
            .map_err(|error| JsValue::from_str(&format!("peer: {error}")))?
            .ok_or_else(|| JsValue::from_str("unknown peer"))?;
        let transport = FetchTransport {
            signer,
            allow_internal,
        };
        let report = super::sync(&mut store, &transport, local, target)
            .await
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let value = serde_json::json!({
            "uploaded": report.uploaded,
            "downloaded": report.downloaded,
            "membership_uploaded": report.membership_uploaded,
            "membership_downloaded": report.membership_downloaded,
            "local_high": report.local_high,
            "remote_high": report.remote_high,
        });
        js_sys::JSON::parse(&value.to_string())
    }
}
