//! Invocation broker and Provider connection lifecycle.
use super::ws::{self, Message as WsMessage, WsConnection, WsError};
use crate::{Error, SharedStore, SystemHost, transport};
use harmion_node::access::{Credential, Grants};
use harmion_node::application::Host;
use harmion_node::interpreter::{
    invocation::{
        self, ActiveInvocation, BeginError, CancelOutcome, PrepareCallError, PreparedCall,
    },
    provider_session::SessionTable,
    registry,
    stream::{
        self as input_stream, CancelOutcome as StreamCancelOutcome, ChunkError, OpenError,
        StreamTable,
    },
};
use harmion_node::storage::Store;
use harmion_protocol::AppId;
use harmion_protocol::interpreter::{
    self as wire, AttachParams, CallRequest, Display, InvocationId, InvokeChunkParams,
    InvokeEndParams, InvokeParams, InvokeStream, ProviderFailure, ProviderId, ProviderOutcome,
    RpcMessage, StreamCancel, StreamChunk, StreamEnd, StreamFrame, StreamId, StreamStart,
};
use hyper::upgrade::Upgraded;
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

enum ProviderCommand {
    Send(Json),
    Close,
}
#[derive(Clone)]
struct SenderEntry {
    tx: mpsc::UnboundedSender<ProviderCommand>,
}
enum Outcome {
    Success {
        data: Json,
        display: Option<Display>,
    },
    Failure(ProviderFailure),
    Unavailable,
    Cancelled,
    Unauthorized,
    InvalidResponse,
}

struct Inner {
    sessions: SessionTable,
    invocations: invocation::InvocationTable,
    streams: StreamTable,
    senders: BTreeMap<ProviderId, SenderEntry>,
    waiters: BTreeMap<InvocationId, oneshot::Sender<Outcome>>,
}

#[derive(Default)]
pub struct Service {
    inner: Mutex<Inner>,
}
impl Default for Inner {
    fn default() -> Self {
        Self {
            sessions: SessionTable::new(),
            invocations: invocation::InvocationTable::default_limits(),
            streams: StreamTable::default_limits(),
            senders: Default::default(),
            waiters: Default::default(),
        }
    }
}
impl Service {
    pub fn new() -> Self {
        Self::default()
    }
    fn lock(&self) -> Result<MutexGuard<'_, Inner>, Error> {
        self.inner
            .lock()
            .map_err(|_| Error::Transport("interpreter state poisoned".into()))
    }
    pub fn app_changed(
        &self,
        app: AppId,
        grants: &Grants,
        revoked: bool,
        disallowed: &[ProviderId],
    ) {
        let Ok(mut inner) = self.lock() else { return };
        let mut closes = Vec::new();
        let mut failed: Vec<(oneshot::Sender<Outcome>, Outcome)> = Vec::new();
        let mut removed = std::collections::BTreeSet::new();
        if revoked || !grants.can_provide() {
            for session in inner.sessions.remove_app(app) {
                removed.insert(session.provider);
            }
        } else {
            for provider in disallowed {
                if inner
                    .sessions
                    .get(*provider)
                    .is_some_and(|session| session.app == app)
                {
                    inner.sessions.remove(*provider);
                    removed.insert(*provider);
                }
            }
        }
        for provider in removed {
            if let Some(sender) = inner.senders.remove(&provider) {
                closes.push(sender);
            }
            for invocation in inner.invocations.fail_provider(provider) {
                if let Some(waiter) = inner.waiters.remove(&invocation.id) {
                    failed.push((
                        waiter,
                        if revoked {
                            Outcome::Unauthorized
                        } else {
                            Outcome::Unavailable
                        },
                    ));
                }
            }
        }
        if revoked || !grants.can_call() {
            // Staged input has not reached a Provider yet; dropping it is enough.
            inner.streams.remove_app(app);
            for invocation in inner.invocations.remove_app(app) {
                if let Some(waiter) = inner.waiters.remove(&invocation.id) {
                    failed.push((
                        waiter,
                        if revoked {
                            Outcome::Unauthorized
                        } else {
                            Outcome::Cancelled
                        },
                    ));
                }
            }
        }
        drop(inner);
        for sender in closes {
            let _ = sender.tx.send(ProviderCommand::Close);
        }
        for (waiter, outcome) in failed {
            let _ = waiter.send(outcome);
        }
    }

    pub async fn handle<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        operation: &str,
        body: Vec<u8>,
    ) -> Result<Json, Error> {
        match operation {
            "interpreter-list" if body.is_empty() => self.list(store, credential).await,
            "interpreter-describe" => {
                let provider: [u8; 32] = body
                    .as_slice()
                    .try_into()
                    .map_err(|_| harmion_node::Error::InvalidRequest)?;
                self.describe(store, credential, ProviderId(provider)).await
            }
            "interpreter-register" => {
                let registration = wire::decode_registration(&body)?;
                let registration = store
                    .run(move |s| registry::register(s, &mut SystemHost, &credential, registration))
                    .await?;
                Ok(json!({
                    "provider_id": registration.provider,
                    "name": registration.name,
                    "contract_version": registration.contract_version,
                    "status": registration.status.as_str(),
                }))
            }
            "interpreter-approve" => self.approve(store, credential, &body).await,
            "interpreter-call" => {
                let request = wire::decode_call_request(&body)?;
                self.call(store, credential, request).await
            }
            "interpreter-call-stream" => {
                let frame = wire::decode_stream_frame(&body)?;
                self.stream(store, credential, frame).await
            }
            "interpreter-cancel" => {
                let id: [u8; 32] = body
                    .as_slice()
                    .try_into()
                    .map_err(|_| harmion_node::Error::InvalidRequest)?;
                self.cancel(store, credential, InvocationId(id)).await
            }
            _ => Err(harmion_node::Error::InvalidRequest.into()),
        }
    }

    async fn list<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
    ) -> Result<Json, Error> {
        let registrations = store
            .run(move |s| registry::list(s, &credential, SystemHost.now_micros()))
            .await?;
        let inner = self.lock()?;
        let providers = registrations
            .iter()
            .map(|registration| {
                registration
                    .summary(inner.sessions.is_connected(registration.provider))
                    .map_err(Error::Node)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"providers": providers}))
    }

    async fn describe<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        provider: ProviderId,
    ) -> Result<Json, Error> {
        let registration = store
            .run(move |s| registry::describe(s, &credential, SystemHost.now_micros(), provider))
            .await?;
        let connected = self.lock()?.sessions.is_connected(provider);
        Ok(serde_json::to_value(registration.describe(connected)?)
            .map_err(|_| harmion_node::Error::InvalidRequest)?)
    }

    async fn approve<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        body: &[u8],
    ) -> Result<Json, Error> {
        let (provider, replace) = decode_approve(body)?;
        let (registration, replaced) = store
            .run(move |s| registry::approve(s, &mut SystemHost, &credential, provider, replace))
            .await?;
        if let Some(provider) = replaced {
            self.close_provider(provider);
        }
        Ok(json!({
            "provider_id": registration.provider,
            "name": registration.name,
            "status": registration.status.as_str(),
            "approved": true,
        }))
    }

    async fn call<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        request: CallRequest,
    ) -> Result<Json, Error> {
        let prepared = store
            .run(move |s| invocation::prepare_call(s, &mut SystemHost, &credential, &request))
            .await?;
        self.dispatch(prepared, None).await
    }

    async fn stream<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        frame: StreamFrame,
    ) -> Result<Json, Error> {
        match frame {
            StreamFrame::Start(start) => self.stream_start(store, credential, start).await,
            StreamFrame::Chunk(chunk) => self.stream_chunk(store, credential, chunk).await,
            StreamFrame::End(end) => self.stream_end(store, credential, end).await,
            StreamFrame::Cancel(cancel) => self.stream_cancel(store, credential, cancel).await,
        }
    }

    async fn stream_start<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        start: StreamStart,
    ) -> Result<Json, Error> {
        // Validate the Provider registration and the caller's grants before any
        // bytes are staged. Contents are materialized only when the stream ends.
        let request = CallRequest {
            provider: start.provider,
            input: start.input,
            contents: Vec::new(),
            timeout_ms: start.timeout_ms,
        };
        let prepared = store
            .run(move |s| invocation::prepare_call(s, &mut SystemHost, &credential, &request))
            .await?;
        let now = SystemHost.now_micros();
        let staged = input_stream::StagedStream {
            id: start.stream_id,
            app: prepared.caller,
            provider: prepared.provider,
            provider_name: prepared.provider_name,
            contract_version: prepared.contract_version,
            input: prepared.input,
            timeout_ms: prepared.timeout_ms,
            started_at: now,
            deadline: now
                .saturating_add((wire::INPUT_STREAM_TIMEOUT_MS as i64).saturating_mul(1000)),
            next_seq: 0,
            contents: Vec::new(),
            seen: Default::default(),
            accepted_bytes: 0,
            accepted_contents: 0,
        };
        {
            let mut inner = self.lock()?;
            // Reclaim abandoned uploads so expired slots do not hold capacity.
            inner.streams.expire(now);
            inner.streams.open(staged).map_err(|error| match error {
                OpenError::NodeCapacity | OpenError::AppCapacity => {
                    Error::from(harmion_node::Error::InterpreterCapacity)
                }
                OpenError::Duplicate => Error::from(harmion_node::Error::InvalidRequest),
            })?;
        }
        Ok(json!({
            "stream_id": start.stream_id,
            "status": "open",
            "limit_bytes": wire::MAX_STREAM_INPUT_BYTES,
            "expires_in_ms": wire::INPUT_STREAM_TIMEOUT_MS,
        }))
    }

    async fn stream_chunk<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        chunk: StreamChunk,
    ) -> Result<Json, Error> {
        let caller = store
            .run(move |s| registry::caller(s, &credential, SystemHost.now_micros()))
            .await?;
        if !caller.grants.can_call() {
            return Err(harmion_node::Error::Forbidden.into());
        }
        let chunk_bytes = serde_json::to_vec(&chunk.contents)
            .map_err(|_| harmion_node::Error::InvalidRequest)?
            .len() as u64;
        let result = {
            let mut inner = self.lock()?;
            inner.streams.chunk(
                chunk.stream_id,
                caller.app,
                chunk.seq,
                chunk.contents,
                chunk_bytes,
                SystemHost.now_micros(),
            )
        };
        match result {
            Ok((next_seq, accepted_contents, accepted_bytes)) => Ok(json!({
                "stream_id": chunk.stream_id,
                "status": "accepted",
                "seq": chunk.seq,
                "next_seq": next_seq,
                "accepted_contents": accepted_contents,
                "accepted_bytes": accepted_bytes,
                "limit_bytes": wire::MAX_STREAM_INPUT_BYTES,
            })),
            Err(error) => chunk_error(chunk.stream_id, error),
        }
    }

    async fn stream_end<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        end: StreamEnd,
    ) -> Result<Json, Error> {
        let auth = credential.clone();
        let caller = store
            .run(move |s| registry::caller(s, &auth, SystemHost.now_micros()))
            .await?;
        if !caller.grants.can_call() {
            return Err(harmion_node::Error::Forbidden.into());
        }
        let taken = {
            let mut inner = self.lock()?;
            inner
                .streams
                .take(end.stream_id, caller.app, SystemHost.now_micros())
        };
        let staged = match taken {
            Ok(staged) => staged,
            Err(error) => return chunk_error(end.stream_id, error),
        };
        let stream_id = staged.id;
        let accepted_contents = staged.accepted_contents;
        let accepted_bytes = staged.accepted_bytes;
        let request = CallRequest {
            provider: staged.provider_name,
            input: staged.input,
            contents: staged.contents,
            timeout_ms: staged.timeout_ms,
        };
        let prepared = store
            .run(move |s| {
                Ok::<_, harmion_node::Error>(invocation::prepare_stream_call(
                    s,
                    &mut SystemHost,
                    &credential,
                    &request,
                ))
            })
            .await?;
        match prepared {
            Ok(prepared) => self.dispatch(prepared, Some(stream_id)).await,
            Err(PrepareCallError::InputTooLarge {
                actual_bytes,
                limit_bytes,
            }) => Ok(stream_error(
                stream_id,
                "stream_limit_exceeded",
                "streamed Provider input exceeds the logical limit",
                json!({
                    "partial": true,
                    "actual_bytes": actual_bytes,
                    "limit_bytes": limit_bytes,
                    "accepted_contents": accepted_contents,
                    "accepted_bytes": accepted_bytes,
                }),
            )),
            Err(PrepareCallError::Call(error)) => Err(error.into()),
        }
    }

    async fn stream_cancel<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        cancel: StreamCancel,
    ) -> Result<Json, Error> {
        let caller = store
            .run(move |s| registry::caller(s, &credential, SystemHost.now_micros()))
            .await?;
        if !caller.grants.can_call() {
            return Err(harmion_node::Error::Forbidden.into());
        }
        let outcome = {
            let mut inner = self.lock()?;
            inner.streams.cancel(cancel.stream_id, caller.app)
        };
        match outcome {
            StreamCancelOutcome::Canceled(id) => Ok(json!({
                "stream_id": id,
                "status": "canceled",
                "canceled": true,
            })),
            StreamCancelOutcome::NotFound => Ok(stream_error(
                cancel.stream_id,
                "stream_not_found",
                "stream is unknown, expired, or already ended",
                Json::Null,
            )),
            StreamCancelOutcome::Forbidden => Err(harmion_node::Error::Forbidden.into()),
        }
    }

    async fn dispatch(
        &self,
        prepared: PreparedCall,
        stream_id: Option<StreamId>,
    ) -> Result<Json, Error> {
        let id = InvocationId(SystemHost.random_bytes()?);
        let PreparedCall {
            caller,
            provider,
            provider_name,
            contract_version,
            input,
            contents,
            content_bytes,
            timeout_ms,
        } = prepared;
        let messages = invoke_messages(
            id,
            provider_name,
            contract_version,
            input,
            contents,
            content_bytes,
            stream_id.is_some(),
        )?;
        let timeout = Duration::from_millis(timeout_ms);
        let now = SystemHost.now_micros();
        let deadline = now.saturating_add((timeout_ms as i64).saturating_mul(1000));
        let (out_tx, mut out_rx) = oneshot::channel();
        let sender = {
            let mut inner = self.lock()?;
            let sender = inner
                .senders
                .get(&provider)
                .cloned()
                .ok_or(harmion_node::Error::InterpreterUnavailable)?;
            inner
                .invocations
                .begin(ActiveInvocation {
                    id,
                    app: caller,
                    provider,
                    started_at: now,
                    deadline,
                })
                .map_err(begin_error)?;
            inner.waiters.insert(id, out_tx);
            sender
        };
        for message in messages {
            if sender.tx.send(ProviderCommand::Send(message)).is_err() {
                self.abort_local(id);
                return Err(harmion_node::Error::InterpreterUnavailable.into());
            }
        }
        let mut lease = Lease {
            service: self,
            id,
            armed: true,
        };
        let result = tokio::time::timeout(timeout, &mut out_rx).await;
        match result {
            Ok(Ok(outcome)) => {
                lease.armed = false;
                self.respond(id, outcome, stream_id)
            }
            Ok(Err(_)) => {
                lease.armed = false;
                Err(harmion_node::Error::InterpreterUnavailable.into())
            }
            Err(_) => {
                lease.armed = false;
                if self.expire_local(id)? {
                    return Err(harmion_node::Error::InterpreterTimeout.into());
                }
                match out_rx.await {
                    Ok(outcome) => self.respond(id, outcome, stream_id),
                    Err(_) => Err(harmion_node::Error::InterpreterUnavailable.into()),
                }
            }
        }
    }

    fn respond(
        &self,
        id: InvocationId,
        outcome: Outcome,
        stream_id: Option<StreamId>,
    ) -> Result<Json, Error> {
        let mut value = match outcome {
            Outcome::Success { data, display } => json!({
                "invocation_id": id,
                "status": "ok",
                "data": data,
                "display": display,
            }),
            Outcome::Failure(error) => json!({
                "invocation_id": id,
                "status": "error",
                "error": error,
            }),
            Outcome::Unavailable => return Err(harmion_node::Error::InterpreterUnavailable.into()),
            Outcome::Cancelled => return Err(harmion_node::Error::InterpreterCancelled.into()),
            Outcome::Unauthorized => return Err(harmion_node::Error::Unauthenticated.into()),
            Outcome::InvalidResponse => {
                return Err(harmion_node::Error::InterpreterInvalidResponse.into());
            }
        };
        if let Some(stream_id) = stream_id {
            value["stream_id"] = json!(stream_id);
        }
        Ok(value)
    }

    pub(crate) fn close_provider(&self, provider: ProviderId) {
        let (sender, failed) = {
            let Ok(mut inner) = self.lock() else { return };
            inner.sessions.remove(provider);
            let sender = inner.senders.remove(&provider);
            let failed = inner
                .invocations
                .fail_provider(provider)
                .into_iter()
                .filter_map(|invocation| {
                    inner
                        .waiters
                        .remove(&invocation.id)
                        .map(|waiter| (invocation, waiter))
                })
                .collect::<Vec<_>>();
            (sender, failed)
        };
        if let Some(sender) = sender {
            let _ = sender.tx.send(ProviderCommand::Close);
        }
        for (_, waiter) in failed {
            let _ = waiter.send(Outcome::Unavailable);
        }
    }
    fn expire_local(&self, id: InvocationId) -> Result<bool, Error> {
        let mut inner = self.lock()?;
        let Some(active) = inner.invocations.finish(id) else {
            return Ok(false);
        };
        inner.waiters.remove(&id);
        if let Some(sender) = inner.senders.get(&active.provider) {
            let _ = sender.tx.send(ProviderCommand::Send(cancel_message(id)));
        }
        Ok(true)
    }
    fn abort_local(&self, id: InvocationId) {
        let Ok(mut inner) = self.lock() else { return };
        let Some(active) = inner.invocations.finish(id) else {
            return;
        };
        inner.waiters.remove(&id);
        if let Some(sender) = inner.senders.get(&active.provider) {
            let _ = sender.tx.send(ProviderCommand::Send(cancel_message(id)));
        }
    }
    fn complete(
        &self,
        id: InvocationId,
    ) -> Result<Option<(ActiveInvocation, oneshot::Sender<Outcome>)>, Error> {
        let mut inner = self.lock()?;
        let Some(active) = inner.invocations.finish(id) else {
            return Ok(None);
        };
        Ok(inner.waiters.remove(&id).map(|waiter| (active, waiter)))
    }

    async fn cancel<S: Store + Send + 'static>(
        &self,
        store: &SharedStore<S>,
        credential: Credential,
        id: InvocationId,
    ) -> Result<Json, Error> {
        let caller = store
            .run(move |s| registry::caller(s, &credential, SystemHost.now_micros()))
            .await?;
        if !caller.grants.can_call() {
            return Err(harmion_node::Error::Forbidden.into());
        }
        let (active, waiter, sender) = {
            let mut inner = self.lock()?;
            let active = match inner.invocations.cancel(id, caller.app) {
                CancelOutcome::Canceled(active) => active,
                CancelOutcome::NotFound => return Err(harmion_node::Error::NotFound.into()),
                CancelOutcome::Forbidden => return Err(harmion_node::Error::Forbidden.into()),
            };
            let waiter = inner.waiters.remove(&id);
            let sender = inner.senders.get(&active.provider).cloned();
            (active, waiter, sender)
        };
        if let Some(waiter) = waiter {
            let _ = waiter.send(Outcome::Cancelled);
        }
        if let Some(sender) = sender {
            let _ = sender
                .tx
                .send(ProviderCommand::Send(cancel_message(active.id)));
        }
        Ok(json!({"canceled":true}))
    }

    /// Serve one authenticated Provider connection until it closes or is replaced.
    pub async fn serve_provider<S: Store + Send + 'static>(
        self: Arc<Self>,
        store: SharedStore<S>,
        credential: Credential,
        upgrade: Upgraded,
    ) -> Result<(), Error> {
        let mut ws = WsConnection::accept(upgrade).await.map_err(ws_error)?;
        let Some((request_id, provider_id)) = read_attach(&mut ws).await? else {
            return Ok(());
        };
        let now = SystemHost.now_micros();
        let registration = match store
            .run(move |s| registry::authorize_attach(s, &credential, now, provider_id))
            .await
        {
            Ok(registration) => registration,
            Err(error) => {
                reject_attach(&mut ws, &request_id, &error).await;
                return Ok(());
            }
        };
        self.serve_attached(ws, request_id, provider_id, registration)
            .await
    }

    /// Browser-origin Provider connection authorized by a single-use ticket.
    pub(crate) async fn serve_provider_ticket<S: Store + Send + 'static>(
        self: Arc<Self>,
        store: SharedStore<S>,
        ticket: crate::oauth_browser::ProviderTicket,
        upgrade: Upgraded,
    ) -> Result<(), Error> {
        let mut ws = WsConnection::accept(upgrade).await.map_err(ws_error)?;
        let Some((request_id, provider_id)) = read_attach(&mut ws).await? else {
            return Ok(());
        };
        if provider_id != ticket.provider {
            let error = wire::rpc_error(&request_id, wire::RPC_NOT_APPROVED, "provider mismatch");
            let _ = send_json(&mut ws, &error).await;
            let _ = ws.send_close(1008, "attach rejected").await;
            return Ok(());
        }
        let now = SystemHost.now_micros();
        let registration = match store
            .run(move |s| registry::authorize_attach_app(s, ticket.app, now, ticket.provider))
            .await
        {
            Ok(registration) => registration,
            Err(error) => {
                reject_attach(&mut ws, &request_id, &error).await;
                return Ok(());
            }
        };
        self.serve_attached(ws, request_id, provider_id, registration)
            .await
    }

    async fn serve_attached(
        self: Arc<Self>,
        mut ws: WsConnection,
        request_id: Json,
        provider_id: ProviderId,
        registration: harmion_node::interpreter::Registration,
    ) -> Result<(), Error> {
        let now = SystemHost.now_micros();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (generation, replaced, failed) = {
            let mut inner = self.lock()?;
            let attached = inner.sessions.attach(provider_id, registration.app, now);
            let old_sender = inner
                .senders
                .insert(provider_id, SenderEntry { tx: tx.clone() });
            let failed = if attached.replaced.is_some() {
                inner
                    .invocations
                    .fail_provider(provider_id)
                    .into_iter()
                    .filter_map(|invocation| {
                        inner
                            .waiters
                            .remove(&invocation.id)
                            .map(|waiter| (invocation, waiter))
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            (attached.session.generation, old_sender, failed)
        };
        if let Some(sender) = replaced {
            let _ = sender.tx.send(ProviderCommand::Close);
        }
        for (_, waiter) in failed {
            let _ = waiter.send(Outcome::Unavailable);
        }
        let attached = json!({
            "ok": true,
            "provider_id": provider_id,
            "name": registration.name,
            "contract_version": registration.contract_version,
        });
        if tx
            .send(ProviderCommand::Send(wire::rpc_result(
                &request_id,
                attached,
            )))
            .is_err()
        {
            return Err(harmion_node::Error::InterpreterUnavailable.into());
        }
        let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    if ws.send_ping().await.is_err() {
                        break;
                    }
                }
                incoming = ws.read_message() => {
                    match incoming {
                        Ok(WsMessage::Text(text)) => {
                            if !self.handle_provider_text(&mut ws, &tx, &text).await? {
                                break;
                            }
                        }
                        Ok(WsMessage::Binary(_)) => {
                            let _ = ws.send_close(1003, "text messages required").await;
                            break;
                        }
                        Ok(WsMessage::Close(..)) => break,
                        Err(WsError::TooLarge) => {
                            let _ = ws.send_close(1009, "message too large").await;
                            break;
                        }
                        Err(WsError::InvalidUtf8) => {
                            let _ = ws.send_close(1007, "invalid UTF-8").await;
                            break;
                        }
                        Err(WsError::Protocol(_)) => {
                            let _ = ws.send_close(1002, "protocol error").await;
                            break;
                        }
                        Err(_) => break,
                    }
                }
                outgoing = rx.recv() => {
                    match outgoing {
                        Some(ProviderCommand::Send(value)) => {
                            if send_json(&mut ws, &value).await.is_err() {
                                break;
                            }
                        }
                        Some(ProviderCommand::Close) | None => {
                            let _ = ws.send_close(1000, "replaced").await;
                            break;
                        }
                    }
                }
            }
        }
        let failed = {
            let mut inner = self.lock()?;
            if inner.sessions.detach(provider_id, generation) {
                inner.senders.remove(&provider_id);
                inner
                    .invocations
                    .fail_provider(provider_id)
                    .into_iter()
                    .filter_map(|invocation| {
                        inner
                            .waiters
                            .remove(&invocation.id)
                            .map(|waiter| (invocation, waiter))
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        };
        for (_, waiter) in failed {
            let _ = waiter.send(Outcome::Unavailable);
        }
        let _ = ws.send_close(1000, "").await;
        Ok(())
    }

    /// Returns false when the connection must close.
    async fn handle_provider_text(
        &self,
        ws: &mut WsConnection,
        tx: &mpsc::UnboundedSender<ProviderCommand>,
        text: &str,
    ) -> Result<bool, Error> {
        let message = match wire::decode_rpc(text.as_bytes()) {
            Ok(message) => message,
            Err(_) => {
                let _ = ws.send_close(1002, "invalid JSON-RPC").await;
                return Ok(false);
            }
        };
        match message {
            RpcMessage::Response { id, result } => {
                let Some(id) = decode_invocation_id(&id) else {
                    let _ = ws.send_close(1002, "invalid invocation id").await;
                    return Ok(false);
                };
                let Some((_, waiter)) = self.complete(id)? else {
                    // Late response after cancel, timeout, or replacement.
                    return Ok(true);
                };
                let outcome = match result {
                    Ok(value) => match wire::decode_provider_result(value) {
                        Ok(ProviderOutcome::Success { data, display }) => {
                            Outcome::Success { data, display }
                        }
                        Ok(ProviderOutcome::Error(error)) => Outcome::Failure(error),
                        Err(_) => Outcome::InvalidResponse,
                    },
                    Err(_) => Outcome::InvalidResponse,
                };
                let _ = waiter.send(outcome);
                Ok(true)
            }
            RpcMessage::Request { id, method, .. } => {
                let (code, message) = if method == wire::RPC_ATTACH {
                    (wire::RPC_ALREADY_ATTACHED, "already attached")
                } else {
                    (wire::RPC_METHOD_NOT_FOUND, "method not found")
                };
                let error = wire::rpc_error(&id, code, message);
                let _ = tx.send(ProviderCommand::Send(error));
                Ok(true)
            }
            RpcMessage::Notification { .. } => Ok(true),
        }
    }
}

fn invoke_messages(
    id: InvocationId,
    provider_name: String,
    contract_version: u32,
    input: Json,
    contents: Vec<wire::ProviderContent>,
    content_bytes: u64,
    streamed: bool,
) -> Result<Vec<Json>, Error> {
    let id_json = serde_json::to_value(id).map_err(invalid_request)?;
    let total_contents = contents.len() as u64;
    if !streamed {
        let message = wire::rpc_request(
            &id_json,
            wire::RPC_INVOKE,
            serde_json::to_value(InvokeParams {
                invocation_id: id,
                provider: provider_name,
                contract_version,
                input,
                contents,
                stream: None,
            })
            .map_err(invalid_request)?,
        );
        check_provider_message(&message)?;
        return Ok(vec![message]);
    }

    // Group by encoded item size so each Provider chunk stays well below the
    // Provider WebSocket frame cap. Order is preserved exactly.
    let mut chunks: Vec<Vec<wire::ProviderContent>> = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = 0usize;
    for item in contents {
        let item_bytes = serde_json::to_vec(&item).map_err(invalid_request)?.len();
        if !current.is_empty()
            && current_bytes.saturating_add(item_bytes) > wire::MAX_STREAM_CHUNK_CONTENT_BYTES
        {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.push(item);
        current_bytes = current_bytes.saturating_add(item_bytes);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    let chunk_count = chunks.len() as u64;
    if chunk_count > wire::MAX_STREAM_CHUNKS {
        return Err(harmion_node::Error::InterpreterInputTooLarge.into());
    }
    let start = wire::rpc_request(
        &id_json,
        wire::RPC_INVOKE,
        serde_json::to_value(InvokeParams {
            invocation_id: id,
            provider: provider_name,
            contract_version,
            input,
            contents: Vec::new(),
            stream: Some(InvokeStream {
                version: wire::STREAM_INPUT_VERSION,
                chunks: chunk_count,
                total_contents,
                total_bytes: content_bytes,
            }),
        })
        .map_err(invalid_request)?,
    );
    check_provider_message(&start)?;
    let mut messages = Vec::with_capacity(chunks.len() + 2);
    messages.push(start);
    for (seq, chunk) in chunks.into_iter().enumerate() {
        let message = wire::rpc_notification(
            wire::RPC_INVOKE_CHUNK,
            serde_json::to_value(InvokeChunkParams {
                invocation_id: id,
                seq: seq as u64,
                contents: chunk,
            })
            .map_err(invalid_request)?,
        );
        check_provider_message(&message)?;
        messages.push(message);
    }
    let end = wire::rpc_notification(
        wire::RPC_INVOKE_END,
        serde_json::to_value(InvokeEndParams {
            invocation_id: id,
            seq: chunk_count,
            total_contents,
            total_bytes: content_bytes,
        })
        .map_err(invalid_request)?,
    );
    check_provider_message(&end)?;
    messages.push(end);
    Ok(messages)
}

fn check_provider_message(message: &Json) -> Result<(), Error> {
    let bytes = serde_json::to_vec(message).map_err(invalid_request)?;
    if bytes.len() > wire::MAX_PROVIDER_MESSAGE_BYTES {
        return Err(harmion_node::Error::InterpreterInputTooLarge.into());
    }
    Ok(())
}

fn invalid_request(_: serde_json::Error) -> Error {
    harmion_node::Error::InvalidRequest.into()
}

fn chunk_error(id: StreamId, error: ChunkError) -> Result<Json, Error> {
    match error {
        ChunkError::NotFound => Ok(stream_error(
            id,
            "stream_not_found",
            "stream is unknown, expired, or already ended",
            Json::Null,
        )),
        ChunkError::Forbidden => Err(harmion_node::Error::Forbidden.into()),
        ChunkError::Expired {
            accepted_contents,
            accepted_bytes,
        } => Ok(stream_error(
            id,
            "stream_expired",
            "the streaming upload deadline passed",
            json!({
                "partial": true,
                "accepted_contents": accepted_contents,
                "accepted_bytes": accepted_bytes,
                "limit_bytes": wire::MAX_STREAM_INPUT_BYTES,
            }),
        )),
        ChunkError::Sequence { expected, received } => Ok(stream_error(
            id,
            "stream_sequence",
            "chunk sequence is not contiguous",
            json!({"expected_seq": expected, "received_seq": received}),
        )),
        ChunkError::Duplicate => Ok(stream_error(
            id,
            "stream_duplicate_content",
            "a Content id is repeated within the stream",
            Json::Null,
        )),
        ChunkError::Limit {
            accepted_contents,
            accepted_bytes,
            limit_bytes,
        } => Ok(stream_error(
            id,
            "stream_limit_exceeded",
            "streamed input exceeds the logical limit",
            json!({
                "partial": true,
                "accepted_contents": accepted_contents,
                "accepted_bytes": accepted_bytes,
                "limit_bytes": limit_bytes,
            }),
        )),
    }
}

fn stream_error(id: StreamId, code: &str, message: &str, details: Json) -> Json {
    let mut error = json!({"code": code, "message": message});
    if let (Some(target), Json::Object(details)) = (error.as_object_mut(), details) {
        for (key, value) in details {
            target.insert(key, value);
        }
    }
    json!({"stream_id": id, "status": "error", "error": error})
}

struct Lease<'a> {
    service: &'a Service,
    id: InvocationId,
    armed: bool,
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.service.abort_local(self.id);
        }
    }
}

async fn read_attach(ws: &mut WsConnection) -> Result<Option<(Json, ProviderId)>, Error> {
    let first = tokio::time::timeout(ws::ATTACH_DEADLINE, ws.read_message())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(ws_error)?;
    let WsMessage::Text(text) = first else {
        let _ = ws.send_close(1002, "attach required").await;
        return Ok(None);
    };
    let (request_id, params) = match wire::decode_rpc(text.as_bytes()) {
        Ok(RpcMessage::Request { id, method, params }) if method == wire::RPC_ATTACH => {
            (id, params)
        }
        _ => {
            let error = wire::rpc_error(&Json::Null, wire::RPC_INVALID_REQUEST, "attach required");
            let _ = send_json(ws, &error).await;
            let _ = ws.send_close(1002, "attach required").await;
            return Ok(None);
        }
    };
    let AttachParams { provider_id } = match wire::decode_attach_params(params) {
        Ok(params) => params,
        Err(_) => {
            let error = wire::rpc_error(
                &request_id,
                wire::RPC_INVALID_PARAMS,
                "invalid attach parameters",
            );
            let _ = send_json(ws, &error).await;
            let _ = ws.send_close(1002, "invalid attach").await;
            return Ok(None);
        }
    };
    Ok(Some((request_id, provider_id)))
}

async fn reject_attach(ws: &mut WsConnection, request_id: &Json, error: &Error) {
    let code = match error {
        Error::Node(harmion_node::Error::InterpreterProviderMismatch) => {
            wire::RPC_PROVIDER_MISMATCH
        }
        Error::Node(
            harmion_node::Error::InterpreterNotApproved
            | harmion_node::Error::NotFound
            | harmion_node::Error::Forbidden,
        ) => wire::RPC_NOT_APPROVED,
        _ => wire::RPC_INTERNAL_ERROR,
    };
    let error = wire::rpc_error(request_id, code, "provider attach rejected");
    let _ = send_json(ws, &error).await;
    let _ = ws.send_close(1008, "attach rejected").await;
}

fn begin_error(error: BeginError) -> harmion_node::Error {
    match error {
        BeginError::NodeCapacity | BeginError::AppCapacity | BeginError::ProviderCapacity => {
            harmion_node::Error::InterpreterCapacity
        }
        BeginError::Duplicate => harmion_node::Error::InvalidRequest,
    }
}
fn cancel_message(id: InvocationId) -> Json {
    wire::rpc_notification(wire::RPC_CANCEL, json!({"invocation_id": id}))
}
fn decode_approve(body: &[u8]) -> Result<(ProviderId, bool), Error> {
    let value = harmion_protocol::decode_app_json(body)?;
    let object = value
        .as_object()
        .ok_or(harmion_node::Error::InvalidRequest)?;
    if object
        .keys()
        .any(|key| key != "provider_id" && key != "replace")
    {
        return Err(harmion_node::Error::InvalidRequest.into());
    }
    let provider = object
        .get("provider_id")
        .cloned()
        .ok_or(harmion_node::Error::InvalidRequest)?;
    let provider: ProviderId =
        serde_json::from_value(provider).map_err(|_| harmion_node::Error::InvalidRequest)?;
    let replace = match object.get("replace") {
        None => false,
        Some(Json::Bool(value)) => *value,
        Some(_) => return Err(harmion_node::Error::InvalidRequest.into()),
    };
    Ok((provider, replace))
}
fn decode_invocation_id(id: &Json) -> Option<InvocationId> {
    let text = id.as_str()?;
    let bytes = harmion_protocol::unhex::<32>(text).ok()?;
    Some(InvocationId(bytes))
}
fn ws_error(error: ws::WsError) -> Error {
    transport(error)
}
async fn send_json(ws: &mut WsConnection, value: &Json) -> Result<(), Error> {
    let text = serde_json::to_string(value).map_err(|_| harmion_node::Error::InvalidRequest)?;
    if text.len() > wire::MAX_PROVIDER_MESSAGE_BYTES {
        return Err(harmion_node::Error::InterpreterInvalidResponse.into());
    }
    ws.send_text(&text).await.map_err(ws_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_narrowing_closes_only_disallowed_sessions() {
        let service = Service::new();
        let app = AppId([1; 32]);
        let allowed = ProviderId([2; 32]);
        let denied = ProviderId([3; 32]);
        let (allowed_tx, mut allowed_rx) = mpsc::unbounded_channel();
        let (denied_tx, mut denied_rx) = mpsc::unbounded_channel();
        {
            let mut inner = service.lock().unwrap();
            inner.sessions.attach(allowed, app, 1);
            inner.sessions.attach(denied, app, 2);
            inner
                .senders
                .insert(allowed, SenderEntry { tx: allowed_tx });
            inner.senders.insert(denied, SenderEntry { tx: denied_tx });
        }
        let grants = Grants {
            provide: true,
            provide_names: ["allowed/name".to_string()].into(),
            ..Grants::default()
        };
        service.app_changed(app, &grants, false, &[denied]);
        {
            let inner = service.lock().unwrap();
            assert!(inner.sessions.is_connected(allowed));
            assert!(!inner.sessions.is_connected(denied));
        }
        assert!(matches!(denied_rx.try_recv(), Ok(ProviderCommand::Close)));
        assert!(allowed_rx.try_recv().is_err());

        service.app_changed(app, &Grants::default(), false, &[]);
        assert!(!service.lock().unwrap().sessions.is_connected(allowed));
        assert!(matches!(allowed_rx.try_recv(), Ok(ProviderCommand::Close)));
    }
}
