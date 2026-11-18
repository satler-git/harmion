//! Separate durable cursor space for membership certificates.
use crate::{Error, membership_state, replication, storage::Transaction};
use harmion_domain::NodeId;
use harmion_protocol::{
    LogEpoch, MAX_PAGE_ENTRIES, MAX_PAGE_RECORD_BYTES, Page, membership::Certificate,
};

pub fn high(tx: &mut dyn Transaction) -> Result<u64, Error> {
    Ok(tx.membership_entries()?.last().map_or(0, |entry| entry.seq))
}
pub fn page(
    tx: &mut dyn Transaction,
    epoch: LogEpoch,
    after: u64,
    high: u64,
) -> Result<Page<Certificate>, Error> {
    if epoch != tx.identity().epoch {
        return Err(Error::IdentityMismatch);
    }
    let entries = tx.membership_entries()?;
    if after > high || high > entries.last().map_or(0, |entry| entry.seq) {
        return Err(Error::InvalidPage);
    }
    let mut result = Vec::new();
    let mut bytes = 0;
    for entry in entries
        .into_iter()
        .filter(|entry| after < entry.seq && entry.seq <= high)
    {
        if result.len() >= MAX_PAGE_ENTRIES
            || bytes + entry.record.bytes().len() > MAX_PAGE_RECORD_BYTES
        {
            break;
        }
        bytes += entry.record.bytes().len();
        result.push(entry);
    }
    Ok(Page {
        epoch,
        after,
        through: result.last().map_or(high, |entry| entry.seq),
        high,
        entries: result,
    })
}
pub fn receive(
    tx: &mut dyn Transaction,
    sender: NodeId,
    page: &Page<Certificate>,
) -> Result<u64, Error> {
    if sender == tx.identity().node {
        return Err(Error::IdentityMismatch);
    }
    let current = tx.membership_cursor(sender, page.epoch)?;
    if current != page.after {
        return Err(Error::CursorMismatch { current });
    }
    replication::validate_page(page, Certificate::bytes)?;
    membership_state::apply(
        tx,
        &page
            .entries
            .iter()
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>(),
    )?;
    tx.set_membership_cursor(sender, page.epoch, page.through)?;
    Ok(page.through)
}
