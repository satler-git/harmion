//! Owner-authorized, fixed export snapshot.

use crate::{
    Error,
    access::{Credential, authenticate},
    storage::{Identity, Store, Transaction, read_content_graph_through},
};
use harmion_domain::{ContentId, Graph};
use harmion_protocol::AppId;

pub struct Snapshot {
    identity: Identity,
    owner: AppId,
    through: u64,
}

impl Snapshot {
    pub fn start(store: &mut impl Store, credential: &Credential, now: i64) -> Result<Self, Error> {
        store.transaction(|tx| {
            let owner = authorize(tx, credential, now)?;
            Ok(Self {
                identity: tx.identity(),
                owner,
                through: tx.accepted_high()?,
            })
        })
    }

    pub fn ids(
        &self,
        store: &mut impl Store,
        credential: &Credential,
        now: i64,
        after: Option<ContentId>,
    ) -> Result<Vec<ContentId>, Error> {
        store.transaction(|tx| {
            self.reauthorize(tx, credential, now)?;
            tx.export_content_ids(self.through, after, 128)
        })
    }

    pub fn content(
        &self,
        store: &mut impl Store,
        credential: &Credential,
        now: i64,
        id: ContentId,
    ) -> Result<Graph, Error> {
        store.transaction(|tx| {
            self.reauthorize(tx, credential, now)?;
            read_content_graph_through(tx, id, Some(self.through))
        })
    }

    fn reauthorize(
        &self,
        tx: &mut dyn Transaction,
        credential: &Credential,
        now: i64,
    ) -> Result<(), Error> {
        if tx.identity() != self.identity || authorize(tx, credential, now)? != self.owner {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

fn authorize(tx: &mut dyn Transaction, credential: &Credential, now: i64) -> Result<AppId, Error> {
    let app = authenticate(tx, credential, now)?.app;
    if !app.grants.is_administrator() {
        return Err(Error::Forbidden);
    }
    Ok(app.id)
}
