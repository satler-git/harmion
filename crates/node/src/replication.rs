//! Transaction-level replication primitives. The network-facing service must
//! authenticate the peer and validate membership before invoking these functions.
use crate::{
    Error,
    storage::{Transaction, accept_records},
};
use harmion_domain::NodeId;
use harmion_protocol::{LogEpoch, MAX_PAGE_ENTRIES, MAX_PAGE_RECORD_BYTES, Page};

pub fn high(tx: &mut dyn Transaction) -> Result<u64, Error> {
    Ok(tx.entries()?.last().map_or(0, |entry| entry.seq))
}

pub fn page(
    tx: &mut dyn Transaction,
    epoch: LogEpoch,
    after: u64,
    high: u64,
) -> Result<Page, Error> {
    if tx.identity().epoch != epoch {
        return Err(Error::IdentityMismatch);
    }
    let log = tx.entries()?;
    if after > high || high > log.last().map_or(0, |entry| entry.seq) {
        return Err(Error::InvalidPage);
    }
    let mut entries = Vec::new();
    let mut bytes = 0;
    for entry in log
        .into_iter()
        .filter(|entry| after < entry.seq && entry.seq <= high)
    {
        if entries.len() == MAX_PAGE_ENTRIES
            || bytes + entry.record.bytes().len() > MAX_PAGE_RECORD_BYTES
        {
            break;
        }
        bytes += entry.record.bytes().len();
        entries.push(entry);
    }
    let through = entries.last().map_or(high, |entry| entry.seq);
    Ok(Page {
        epoch,
        after,
        through,
        high,
        entries,
    })
}

/// Commit the returned cursor before sending any ACK. A stale after asks the caller
/// to re-query the durable cursor; a lost ACK does not cause duplicate log rows.
pub fn receive(tx: &mut dyn Transaction, sender: NodeId, page: &Page) -> Result<u64, Error> {
    if sender == tx.identity().node {
        return Err(Error::IdentityMismatch);
    }
    let current = tx.cursor(sender, page.epoch)?;
    if page.after != current {
        return Err(Error::CursorMismatch { current });
    }
    validate_page(page, harmion_protocol::Record::bytes)?;
    accept_records(
        tx,
        &page
            .entries
            .iter()
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>(),
    )?;
    tx.set_cursor(sender, page.epoch, page.through)?;
    Ok(page.through)
}

pub fn validate_page<T>(page: &Page<T>, record_bytes: fn(&T) -> &[u8]) -> Result<(), Error> {
    if page.after > page.through
        || page.through > page.high
        || page.entries.len() > MAX_PAGE_ENTRIES
    {
        return Err(Error::InvalidPage);
    }
    let mut previous = page.after;
    let mut bytes = 0;
    for entry in &page.entries {
        if entry.seq <= previous || entry.seq > page.through {
            return Err(Error::InvalidPage);
        }
        previous = entry.seq;
        bytes += record_bytes(&entry.record).len();
        if bytes > MAX_PAGE_RECORD_BYTES {
            return Err(Error::InvalidPage);
        }
    }
    if page
        .entries
        .last()
        .map_or(page.through != page.high, |entry| entry.seq != page.through)
    {
        return Err(Error::InvalidPage);
    }
    Ok(())
}
