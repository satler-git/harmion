//! Local application operations. Every entry point authenticates and reads current
//! grants inside its storage transaction; tokens never grant read or write access.
use crate::{
    Error,
    access::{App, Credential, Grants, authenticate},
    storage::{Store, Transaction, read_content_graph},
};
use harmion_domain::*;
use harmion_protocol::{AppId, Command, EditToken, Record, SaveRequest, encode_save_request};
use harmion_protocol::{
    hex,
    resolution::{MergeInput, MergeInputRequest, MergePart},
};
use std::collections::{BTreeMap, BTreeSet};

/// Host-provided entropy and UTC clock; neither is inferred from untrusted requests.
pub trait Host {
    fn random_bytes(&mut self) -> Result<[u8; 32], Error>;
    fn now_micros(&self) -> i64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenPurpose {
    Edit,
    Resolve,
    Manage,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenState {
    pub content_id: ContentId,
    pub basis: BTreeSet<UpdateId>,
    pub purpose: TokenPurpose,
    /// Creation wall time in microseconds; zero marks rows that predate the
    /// retention policy and are kept.
    pub created_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestResult {
    pub request: Vec<u8>,
    pub update_id: UpdateId,
    pub snapshot_token: EditToken,
    pub return_view: bool,
    pub created_at: i64,
}

pub use harmion_protocol::{ConflictView, ReadView, SaveResult};

pub(crate) fn view(snapshot: &ContentSnapshot, token: EditToken, grants: &Grants) -> ReadView {
    ReadView {
        content: snapshot.content.clone(),
        edit_token: token,
        conflicts: snapshot
            .fields
            .iter()
            .filter(|(field, state)| {
                state.is_conflicted()
                    && (!matches!(field, Field::Kind(_)) || grants.is_administrator())
            })
            .map(|(field, _)| field.clone())
            .collect(),
        deletion_conflict: snapshot.life == Life::Conflict,
    }
}

pub(crate) fn mint_token(
    tx: &mut dyn Transaction,
    host: &mut impl Host,
    snapshot: &ContentSnapshot,
    purpose: TokenPurpose,
) -> Result<EditToken, Error> {
    let token = EditToken(host.random_bytes()?);
    tx.insert_token(
        token,
        &TokenState {
            content_id: snapshot.content.id,
            basis: snapshot.basis.clone(),
            purpose,
            created_at: host.now_micros(),
        },
    )?;
    Ok(token)
}

/// Only the owner-controlled initialization path may call this; no network route.
/// Revoked or expired apps still count, so deleting grants cannot re-enable bootstrap.
pub fn bootstrap_admin(
    store: &mut impl Store,
    host: &mut impl Host,
    expires_at: i64,
) -> Result<(AppId, Credential), Error> {
    store.transaction(|tx| {
        if tx.has_apps()? {
            return Err(Error::AlreadyBootstrapped);
        }
        insert_app(tx, host, Grants::administrator(), expires_at)
    })
}
fn insert_app(
    tx: &mut dyn Transaction,
    host: &mut impl Host,
    grants: Grants,
    expires_at: i64,
) -> Result<(AppId, Credential), Error> {
    if expires_at <= host.now_micros() {
        return Err(Error::InvalidRequest);
    }
    let id = AppId(host.random_bytes()?);
    let credential = Credential::from_bytes(host.random_bytes()?);
    tx.insert_app(&App {
        id,
        verifier: credential.verifier(),
        expires_at,
        revoked: false,
        grants,
    })?;
    Ok((id, credential))
}
pub fn issue_app(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    grants: Grants,
    expires_at: i64,
) -> Result<(AppId, Credential), Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        if !principal.app.grants.manage_apps {
            return Err(Error::Forbidden);
        }
        insert_app(tx, host, grants, expires_at)
    })
}
pub fn change_grants(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    id: AppId,
    grants: Grants,
    revoked: bool,
) -> Result<(), Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, now)?;
        if !principal.app.grants.manage_apps {
            return Err(Error::Forbidden);
        }
        let mut app = tx.app_by_id(id)?.ok_or(Error::NotFound)?;
        if app.grants == grants && app.revoked == revoked {
            return Ok(());
        }
        if tx.oauth_client(app.id)?.is_some() {
            crate::oauth::restrict_grant_update(&app.grants, &grants)?;
            if revoked {
                tx.revoke_oauth_app(app.id)?;
            }
        }
        app.grants = grants;
        app.revoked = revoked;
        tx.update_app(&app)
    })
}

pub fn get(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    id: ContentId,
) -> Result<ReadView, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        let snapshot = read_content_graph(tx, id)?
            .snapshot(id)
            .ok_or(Error::NotFound)?;
        if !principal.app.grants.can_read_snapshot(&snapshot) {
            return Err(Error::NotFound);
        }
        // Reject before minting a durable token or cloning the output. The
        // snapshot and visibility checks still use the complete stored history.
        harmion_protocol::snapshot_read_view_size(
            &snapshot,
            principal.app.grants.is_administrator(),
            harmion_protocol::MAX_APP_RESPONSE_BYTES,
        )
        .ok_or(Error::ResponseTooLarge)?;
        let token = mint_token(tx, host, &snapshot, TokenPurpose::Edit)?;
        Ok(view(&snapshot, token, &principal.app.grants))
    })
}

pub fn get_conflicts(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    id: ContentId,
) -> Result<ConflictView, Error> {
    read_conflicts(store, host, credential, id, false)
}
/// Owner management tool can inspect deleted contents and obtain a restore token.
pub fn inspect(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    id: ContentId,
) -> Result<ConflictView, Error> {
    read_conflicts(store, host, credential, id, true)
}

pub fn metrics(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
) -> Result<harmion_protocol::metrics::NodeMetrics, Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?
            .app
            .grants
            .is_administrator()
        {
            return Err(Error::Forbidden);
        }
        tx.node_metrics(now)
    })
}
fn read_conflicts(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    id: ContentId,
    management: bool,
) -> Result<ConflictView, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        let grants = &principal.app.grants;
        if management && !grants.is_administrator() {
            return Err(Error::Forbidden);
        }
        let snapshot = read_content_graph(tx, id)?
            .snapshot(id)
            .ok_or(Error::NotFound)?;
        if !grants.can_read(&snapshot.content) || (!management && snapshot.life == Life::Deleted) {
            return Err(Error::NotFound);
        }
        harmion_protocol::snapshot_conflict_view_size(
            &snapshot,
            grants.is_administrator(),
            harmion_protocol::MAX_APP_RESPONSE_BYTES,
        )
        .ok_or(Error::ResponseTooLarge)?;
        let purpose = if management {
            TokenPurpose::Manage
        } else {
            TokenPurpose::Resolve
        };
        let token = mint_token(tx, host, &snapshot, purpose)?;
        Ok(ConflictView {
            view: view(&snapshot, token, grants),
            deleted: snapshot.life == Life::Deleted,
            candidates: snapshot
                .fields
                .iter()
                .filter(|(field, _)| !matches!(field, Field::Kind(_)) || grants.is_administrator())
                .map(|(field, state)| (field.clone(), state.candidates.clone()))
                .collect(),
        })
    })
}

fn token_snapshot(
    tx: &mut dyn Transaction,
    graph: &Graph,
    token: EditToken,
    content: ContentId,
) -> Result<(TokenPurpose, ContentSnapshot), Error> {
    let state = tx.token(token)?.ok_or(Error::TokenExpired)?;
    if state.content_id != content {
        return Err(Error::InvalidToken);
    }
    Ok((state.purpose, graph.snapshot_at(content, &state.basis)?))
}

fn candidate_value<'a>(
    snapshot: &'a ContentSnapshot,
    field: &Field,
    version: UpdateId,
) -> Option<&'a Value> {
    let state = snapshot.fields.get(field)?;
    state
        .candidates
        .iter()
        .find(|candidate| candidate.versions.contains(&version))
        .map(|candidate| &candidate.value)
}

/// Historical bodies are available only to the owner for two body versions in
/// the fixed management snapshot. Concurrent arrivals cannot change the pair.
pub fn merge_input(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
    request: MergeInputRequest,
) -> Result<MergeInput, Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?
            .app
            .grants
            .is_administrator()
        {
            return Err(Error::Forbidden);
        }
        let graph = read_content_graph(tx, request.content)?;
        let (purpose, snapshot) = token_snapshot(tx, &graph, request.token, request.content)?;
        if purpose != TokenPurpose::Manage {
            return Err(Error::InvalidToken);
        }
        let body = snapshot.fields.get(&Field::Body).ok_or(Error::CorruptLog)?;
        let is_candidate = |version| {
            body.candidates
                .iter()
                .any(|c| c.versions.contains(&version))
        };
        if request.left == request.right
            || !is_candidate(request.left)
            || !is_candidate(request.right)
        {
            return Err(Error::InvalidRequest);
        }
        let version = match request.part {
            MergePart::Left => request.left,
            MergePart::Right => request.right,
            MergePart::Base(selected) => {
                let bases = graph.common_body_versions(request.left, request.right)?;
                match selected {
                    Some(version) if bases.contains(&version) => version,
                    Some(_) => return Err(Error::InvalidRequest),
                    None if bases.is_empty() => return Ok(MergeInput::NoBase),
                    None if bases.len() == 1 => *bases.first().expect("one base"),
                    None => {
                        return Ok(MergeInput::ChooseBase {
                            versions: bases.into_iter().map(|version| hex(&version.0)).collect(),
                        });
                    }
                }
            }
        };
        let Some(Value::Body(text)) = graph
            .update(version)
            .and_then(|update| update.changes.get(&Field::Body))
            .map(|change| &change.value)
        else {
            return Err(Error::CorruptLog);
        };
        Ok(MergeInput::Body {
            version: hex(&version.0),
            text: text.clone(),
        })
    })
}

fn save_response(
    tx: &mut dyn Transaction,
    graph: &Graph,
    result: &RequestResult,
    content: ContentId,
    grants: &Grants,
) -> Result<SaveResult, Error> {
    let (_, saved) = token_snapshot(tx, graph, result.snapshot_token, content)?;
    let current = graph.snapshot(content).ok_or(Error::NotFound)?;
    let readable = result.return_view
        && saved.life != Life::Deleted
        && current.life != Life::Deleted
        && grants.can_read(&saved.content)
        && grants.can_read(&current.content);
    Ok(SaveResult {
        update_id: result.update_id,
        view: readable.then(|| view(&saved, result.snapshot_token, grants)),
    })
}

pub fn save(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    request: &SaveRequest,
) -> Result<SaveResult, Error> {
    save_with_outcome(store, host, credential, request).map(|(result, _)| result)
}

/// Returns the receipt and whether the transaction committed a new update.
pub fn save_with_outcome(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    request: &SaveRequest,
) -> Result<(SaveResult, bool), Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        let grants = &principal.app.grants;
        let bytes = encode_save_request(request)?;
        let mut graph = read_content_graph(tx, request.content_id)?;
        if let Some(result) = tx.request(principal.app.id, request.request_id)? {
            if result.request != bytes {
                return Err(Error::RequestIdReuse);
            }
            // No new write. Reauthorize response content against both saved and current kinds.
            return Ok((
                save_response(tx, &graph, &result, request.content_id, grants)?,
                false,
            ));
        }
        let before = graph.snapshot(request.content_id);
        if let Some(snapshot) = &before
            && !grants.can_write(&snapshot.content)
        {
            return Err(Error::Forbidden);
        }
        let fixed = request
            .command
            .token()
            .map(|token| token_snapshot(tx, &graph, token, request.content_id))
            .transpose()?;
        let update_id = UpdateId(host.random_bytes()?);
        let mut update = Update {
            format_version: 1,
            cluster_id: tx.identity().cluster,
            content_id: request.content_id,
            root_id: update_id,
            update_id,
            author_node_id: tx.identity().node,
            edited_at: host.now_micros(),
            action: Action::Create,
            basis: BTreeSet::new(),
            changes: BTreeMap::new(),
        };
        if let Some((_, snapshot)) = &fixed {
            update.basis = snapshot.basis.clone();
            update.root_id = graph
                .update(*snapshot.basis.first().ok_or(Error::InvalidToken)?)
                .ok_or(Error::InvalidToken)?
                .root_id;
        }
        match &request.command {
            Command::Create { fields } => {
                if before.is_some() {
                    return Err(Error::InvalidRequest);
                }
                update.changes = fields
                    .iter()
                    .map(|(field, value)| {
                        (
                            field.clone(),
                            Change {
                                value: value.clone(),
                                replaces: BTreeSet::new(),
                            },
                        )
                    })
                    .collect();
            }
            Command::Edit { fields, .. } => {
                let (purpose, snapshot) = fixed.as_ref().ok_or(Error::InvalidToken)?;
                if !matches!(purpose, TokenPurpose::Edit | TokenPurpose::Manage) {
                    return Err(Error::InvalidToken);
                }
                if fields.is_empty() {
                    return Err(Error::InvalidRequest);
                }
                update.action = Action::Edit;
                let parents = snapshot.edit_parents();
                for (field, value) in fields {
                    if matches!(field, Field::Kind(_))
                        && !grants.is_administrator()
                        && (snapshot
                            .fields
                            .get(field)
                            .is_some_and(FieldState::is_conflicted)
                            || before
                                .as_ref()
                                .and_then(|s| s.fields.get(field))
                                .is_some_and(FieldState::is_conflicted))
                    {
                        return Err(Error::Forbidden);
                    }
                    update.changes.insert(
                        field.clone(),
                        Change {
                            value: value.clone(),
                            replaces: parents.get(field).cloned().unwrap_or_default(),
                        },
                    );
                }
            }
            Command::Resolve { .. } | Command::Select { .. } => {
                let (purpose, snapshot) = fixed.as_ref().ok_or(Error::InvalidToken)?;
                if !matches!(purpose, TokenPurpose::Resolve | TokenPurpose::Manage) {
                    return Err(Error::InvalidToken);
                }
                let changes = match &request.command {
                    Command::Resolve { changes, .. } => changes.clone(),
                    Command::Select { selections, .. } => selections
                        .iter()
                        .map(|(field, selection)| {
                            if matches!(field, Field::Kind(_)) && !grants.is_administrator() {
                                return Err(Error::Forbidden);
                            }
                            let value = candidate_value(snapshot, field, selection.version)
                                .filter(|_| selection.replaces.contains(&selection.version))
                                .ok_or(Error::InvalidRequest)?;
                            Ok((
                                field.clone(),
                                Change {
                                    value: value.clone(),
                                    replaces: selection.replaces.clone(),
                                },
                            ))
                        })
                        .collect::<Result<_, Error>>()?,
                    _ => unreachable!(),
                };
                if changes.is_empty() {
                    return Err(Error::InvalidRequest);
                }
                for (field, change) in &changes {
                    if matches!(field, Field::Kind(_)) && !grants.is_administrator() {
                        return Err(Error::Forbidden);
                    }
                    let state = snapshot.fields.get(field).ok_or(Error::InvalidRequest)?;
                    let allowed: BTreeSet<_> = state
                        .candidates
                        .iter()
                        .flat_map(|c| c.versions.iter().copied())
                        .collect();
                    if change.replaces.is_empty() || !change.replaces.is_subset(&allowed) {
                        return Err(Error::InvalidRequest);
                    }
                }
                update.action = Action::Edit;
                update.changes = changes;
            }
            Command::Delete { .. } => {
                let (_, snapshot) = fixed.as_ref().ok_or(Error::InvalidToken)?;
                if !grants.is_administrator()
                    && (snapshot.life == Life::Conflict
                        || before.as_ref().is_some_and(|s| s.life == Life::Conflict))
                {
                    return Err(Error::Forbidden);
                }
                update.action = Action::Delete;
            }
            Command::Keep { .. } | Command::Restore { .. } | Command::RestoreFrom { .. } => {
                let (purpose, snapshot) = fixed.as_ref().ok_or(Error::InvalidToken)?;
                if !grants.is_administrator() {
                    return Err(Error::Forbidden);
                }
                if *purpose != TokenPurpose::Manage {
                    return Err(Error::InvalidToken);
                }
                match &request.command {
                    Command::Restore { fields, .. } => {
                        update.action = Action::Restore;
                        update.changes = graph.restore_changes(&update.basis, fields.clone())?;
                    }
                    Command::RestoreFrom { body, fields, .. } => {
                        let mut values = BTreeMap::from([(Field::Body, Value::Body(body.clone()))]);
                        for (field, version) in fields {
                            let value = candidate_value(snapshot, field, *version)
                                .ok_or(Error::InvalidRequest)?;
                            values.insert(field.clone(), value.clone());
                        }
                        update.action = Action::Restore;
                        update.changes = graph.restore_changes(&update.basis, values)?;
                    }
                    _ => update.action = Action::Keep,
                }
            }
        }
        let record = Record::from_update(update)?;
        if tx.accepted_record(update_id)?.is_some() {
            return Err(Error::Domain(harmion_domain::Error::IdMismatch));
        }
        if graph.accept(record.update().clone())? != Acceptance::Inserted {
            return Err(Error::InvalidRequest);
        }
        let after = graph.snapshot(request.content_id).ok_or(Error::NotFound)?;
        if !grants.can_write(&after.content) {
            return Err(Error::Forbidden);
        }
        tx.append(&record)?;
        tx.put_search_projection(&after)?;
        let token = mint_token(tx, host, &after, TokenPurpose::Edit)?;
        let result = RequestResult {
            request: bytes,
            update_id,
            snapshot_token: token,
            return_view: after.life != Life::Deleted && grants.can_read(&after.content),
            created_at: host.now_micros(),
        };
        tx.insert_request(principal.app.id, request.request_id, &result)?;
        Ok((
            save_response(tx, &graph, &result, request.content_id, grants)?,
            true,
        ))
    })
}
