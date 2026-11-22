//! Durable Provider registration and owner approval.
use crate::Error;
use crate::access::{Credential, Grants, authenticate};
use crate::application::Host;
use crate::interpreter::{Registration, manifest_bytes};
use crate::storage::{Store, Transaction};
use harmion_protocol::AppId;
use harmion_protocol::interpreter::{ProviderId, ProviderRegistration, ProviderStatus};

pub struct Caller {
    pub app: AppId,
    pub grants: Grants,
}

pub fn caller(store: &mut impl Store, credential: &Credential, now: i64) -> Result<Caller, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, now)?;
        Ok(Caller {
            app: principal.app.id,
            grants: principal.app.grants,
        })
    })
}

/// Register or update this app's Provider manifest. A name pre-approved by the
/// app's own grant (OAuth `provide:NAME`) is approved immediately; otherwise a
/// new or superseded registration stays pending for owner approval.
pub fn register(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    registration: ProviderRegistration,
) -> Result<Registration, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        register_in_tx(tx, host, principal.app.id, registration)
    })
}

/// Transaction-level variant used by the integrated connection approval after
/// the owner has already approved the requested grants.
pub(crate) fn register_in_tx(
    tx: &mut dyn Transaction,
    host: &mut impl Host,
    app: AppId,
    registration: ProviderRegistration,
) -> Result<Registration, Error> {
    let manifest = manifest_bytes(&registration)?;
    let app_record = tx.app_by_id(app)?.ok_or(Error::NotFound)?;
    if !app_record.grants.can_provide()
        || !app_record.grants.allows_provider_name(&registration.name)
    {
        return Err(Error::Forbidden);
    }
    let now = host.now_micros();
    let preapproved = app_record
        .grants
        .is_provider_name_preapproved(&registration.name);
    if preapproved
        && let Some(current) = tx.approved_interpreter_by_name(&registration.name)?
        && current.app != app
    {
        return Err(Error::InterpreterNameConflict);
    }
    let existing = tx.interpreter_registration_by_app_name(app, &registration.name)?;
    let registration = match existing {
        Some(mut existing) => {
            let definition_changed = existing.manifest != manifest
                || existing.contract_version != registration.contract_version;
            existing.manifest = manifest;
            existing.name = registration.name;
            existing.contract_version = registration.contract_version;
            if existing.status == ProviderStatus::Superseded {
                existing.status = ProviderStatus::Pending;
                existing.approved_at = None;
                existing.created_at = now;
            }
            // The owner approves a concrete definition (schema and default
            // query). A replacement must be reviewed again instead of keeping
            // the old approval while new callers see the new contract.
            if definition_changed && !preapproved && existing.status == ProviderStatus::Approved {
                existing.status = ProviderStatus::Pending;
                existing.approved_at = None;
            }
            if preapproved {
                existing.status = ProviderStatus::Approved;
                existing.approved_at = Some(now);
            }
            tx.update_interpreter_registration(&existing)?;
            tx.set_interpreter_status(existing.provider, existing.status, existing.approved_at)?;
            existing
        }
        None => {
            let status = if preapproved {
                ProviderStatus::Approved
            } else {
                ProviderStatus::Pending
            };
            let registration = Registration {
                provider: ProviderId(host.random_bytes()?),
                app,
                name: registration.name,
                contract_version: registration.contract_version,
                manifest,
                status,
                created_at: now,
                approved_at: (status == ProviderStatus::Approved).then_some(now),
            };
            tx.insert_interpreter_registration(&registration)?;
            registration
        }
    };
    Ok(registration)
}

/// Supersede registrations that the current grants no longer authorize. This
/// is called in the same transaction as every grant change.
pub(crate) fn reconcile_grants(
    tx: &mut dyn Transaction,
    app: AppId,
    grants: &Grants,
) -> Result<(), Error> {
    for registration in tx
        .interpreter_registrations()?
        .into_iter()
        .filter(|registration| registration.app == app)
    {
        if registration.status == ProviderStatus::Superseded {
            continue;
        }
        if !grants.allows_provider_name(&registration.name) {
            tx.set_interpreter_status(
                registration.provider,
                ProviderStatus::Superseded,
                registration.approved_at,
            )?;
        }
    }
    Ok(())
}

/// Approve one pending Provider. Name uniqueness is enforced among approved
/// Providers; `replace` explicitly replaces a different approved Provider.
pub fn approve(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    provider: ProviderId,
    replace: bool,
) -> Result<(Registration, Option<ProviderId>), Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        if !principal.app.grants.manage_apps {
            return Err(Error::Forbidden);
        }
        let (registration, replaced) = approve_in_tx(tx, host, provider, replace)?;
        Ok((registration, replaced))
    })
}

pub(crate) fn approve_in_tx(
    tx: &mut dyn Transaction,
    host: &mut impl Host,
    provider: ProviderId,
    replace: bool,
) -> Result<(Registration, Option<ProviderId>), Error> {
    let mut registration = tx
        .interpreter_registration(provider)?
        .ok_or(Error::NotFound)?;
    if registration.status == ProviderStatus::Superseded {
        return Err(Error::InterpreterNotApproved);
    }
    let app = tx
        .app_by_id(registration.app)?
        .ok_or(Error::InterpreterNotApproved)?;
    if app.revoked
        || app.expires_at <= host.now_micros()
        || !app.grants.allows_provider_name(&registration.name)
    {
        return Err(Error::InterpreterNotApproved);
    }
    let mut replaced = None;
    let current = tx.approved_interpreter_by_name(&registration.name)?;
    if let Some(current) = current
        && current.provider != provider
    {
        if !replace {
            return Err(Error::InterpreterNameConflict);
        }
        tx.set_interpreter_status(
            current.provider,
            ProviderStatus::Superseded,
            current.approved_at,
        )?;
        replaced = Some(current.provider);
    }
    if registration.status != ProviderStatus::Approved {
        registration.status = ProviderStatus::Approved;
        registration.approved_at = Some(host.now_micros());
        tx.set_interpreter_status(
            registration.provider,
            registration.status,
            registration.approved_at,
        )?;
    }
    Ok((registration, replaced))
}

/// Approved Providers are visible to every authenticated app. An app also sees
/// its own pending or superseded registrations; only administrators see other
/// apps' unapproved manifests.
pub fn list(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
) -> Result<Vec<Registration>, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, now)?;
        let admin = principal.app.grants.manage_apps || principal.app.grants.is_administrator();
        let own = principal.app.id;
        Ok(tx
            .interpreter_registrations()?
            .into_iter()
            .filter(|registration| {
                registration.status == ProviderStatus::Approved || admin || registration.app == own
            })
            .collect())
    })
}

pub fn describe(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
    provider: ProviderId,
) -> Result<Registration, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, now)?;
        let registration = tx
            .interpreter_registration(provider)?
            .ok_or(Error::NotFound)?;
        let visible = registration.status == ProviderStatus::Approved
            || registration.app == principal.app.id
            || principal.app.grants.manage_apps;
        if visible {
            Ok(registration)
        } else {
            Err(Error::NotFound)
        }
    })
}

/// A Provider connection is accepted only for its owning app, with a current
/// Provide grant and an approved registration.
pub fn authorize_attach(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
    provider: ProviderId,
) -> Result<Registration, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, now)?;
        authorize_attach_for_app_in_tx(tx, principal.app.id, now, provider)
    })
}

/// A browser ticket has no credential at attach time; the ticket binds the app
/// and provider, and this rechecks the current grant and approval.
pub fn authorize_attach_app(
    store: &mut impl Store,
    app: AppId,
    now: i64,
    provider: ProviderId,
) -> Result<Registration, Error> {
    store.transaction(|tx| authorize_attach_for_app_in_tx(tx, app, now, provider))
}

pub(crate) fn authorize_attach_for_app_in_tx(
    tx: &mut dyn Transaction,
    app: AppId,
    now: i64,
    provider: ProviderId,
) -> Result<Registration, Error> {
    let app_record = tx.app_by_id(app)?.ok_or(Error::Unauthenticated)?;
    if app_record.revoked || now >= app_record.expires_at || !app_record.grants.can_provide() {
        return Err(Error::Forbidden);
    }
    let registration = tx
        .interpreter_registration(provider)?
        .ok_or(Error::NotFound)?;
    if registration.app != app {
        return Err(Error::InterpreterProviderMismatch);
    }
    if registration.status != ProviderStatus::Approved {
        return Err(Error::InterpreterNotApproved);
    }
    if !app_record.grants.allows_provider_name(&registration.name) {
        return Err(Error::InterpreterUnavailable);
    }
    registration.decoded()?;
    Ok(registration)
}

/// Providers whose names are no longer allowed by the app's current grants.
/// Callers use this to close already-established Provider sessions after a
/// grant change, including the narrowing case that keeps `provide` true.
pub fn disallowed_providers(
    store: &mut impl Store,
    app: AppId,
    grants: &Grants,
) -> Result<Vec<ProviderId>, Error> {
    store.transaction(|tx| disallowed_in_tx(tx, app, grants))
}

pub(crate) fn disallowed_in_tx(
    tx: &mut dyn Transaction,
    app: AppId,
    grants: &Grants,
) -> Result<Vec<ProviderId>, Error> {
    let mut providers = std::collections::BTreeSet::new();
    for registration in tx
        .interpreter_registrations()?
        .into_iter()
        .filter(|registration| registration.app == app)
    {
        if !grants.allows_provider_name(&registration.name) {
            providers.insert(registration.provider);
        }
    }
    Ok(providers.into_iter().collect())
}
