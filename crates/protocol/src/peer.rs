use crate::{
    CodecError, MAX_RECORD_BYTES, Record,
    record::{Decoder, Encoder},
};
use harmion_domain::{ClusterId, NodeId, UpdateId};

pub const MAX_PAGE_ENTRIES: usize = 128;
pub const MAX_PAGE_RECORD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PEER_BYTES: usize = MAX_PAGE_RECORD_BYTES + 8192;

/// Persisted log numbering generation; distinct from a node or update ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogEpoch(pub [u8; 32]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry<T = Record> {
    pub seq: u64,
    pub record: T,
}

/// Internal Peer DTO. Authentication is supplied separately by the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T = Record> {
    pub epoch: LogEpoch,
    pub after: u64,
    pub through: u64,
    pub high: u64,
    pub entries: Vec<LogEntry<T>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerCommand {
    Status,
    Cursor {
        epoch: LogEpoch,
    },
    Page {
        epoch: LogEpoch,
        after: u64,
        high: u64,
    },
    Accept(Page),
    MembershipStatus,
    MembershipCursor {
        epoch: LogEpoch,
    },
    MembershipPage {
        epoch: LogEpoch,
        after: u64,
        high: u64,
    },
    MembershipAccept(Page<crate::membership::Certificate>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerRequest {
    pub cluster: ClusterId,
    pub node: NodeId,
    pub command: PeerCommand,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerReply {
    MembershipPage(Page<crate::membership::Certificate>),
    Status {
        epoch: LogEpoch,
        high: u64,
    },
    Cursor {
        epoch: LogEpoch,
        through: u64,
    },
    Page(Page),
    Ack {
        epoch: LogEpoch,
        through: u64,
    },
    CursorMismatch {
        epoch: LogEpoch,
        current: u64,
    },
    Rejected {
        update_id: UpdateId,
        reason: Rejection,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    MissingDependency,
    IdMismatch,
    InvalidUpdate,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerResponse {
    pub cluster: ClusterId,
    pub node: NodeId,
    pub reply: PeerReply,
}

fn header(magic: &[u8], cluster: ClusterId, node: NodeId) -> Result<Encoder, CodecError> {
    let mut e = Encoder::with_limit(MAX_PEER_BYTES);
    e.raw(magic)?;
    e.raw(&1u32.to_be_bytes())?;
    e.raw(&cluster.0)?;
    e.raw(&node.0)?;
    Ok(e)
}
fn parse_header<'a>(
    bytes: &'a [u8],
    magic: &[u8],
) -> Result<(Decoder<'a>, ClusterId, NodeId), CodecError> {
    let mut d = Decoder::with_limit(bytes, MAX_PEER_BYTES)?;
    if d.raw(4)? != magic {
        return Err(CodecError::BadMagic);
    }
    let version = d.u32()?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion(version));
    }
    let cluster = ClusterId(d.fixed()?);
    let node = NodeId(d.fixed()?);
    Ok((d, cluster, node))
}
fn put_page<T>(e: &mut Encoder, page: &Page<T>, bytes: fn(&T) -> &[u8]) -> Result<(), CodecError> {
    if page.entries.len() > MAX_PAGE_ENTRIES {
        return Err(CodecError::TooManyItems);
    }
    if page
        .entries
        .iter()
        .map(|entry| bytes(&entry.record).len())
        .sum::<usize>()
        > MAX_PAGE_RECORD_BYTES
    {
        return Err(CodecError::TooLarge);
    }
    e.raw(&page.epoch.0)?;
    for n in [page.after, page.through, page.high] {
        e.raw(&n.to_be_bytes())?;
    }
    e.count(page.entries.len())?;
    for entry in &page.entries {
        e.raw(&entry.seq.to_be_bytes())?;
        e.raw(&(bytes(&entry.record).len() as u32).to_be_bytes())?;
        e.raw(bytes(&entry.record))?;
    }
    Ok(())
}
fn get_page<T>(
    d: &mut Decoder<'_>,
    parse: fn(&[u8]) -> Result<T, CodecError>,
) -> Result<Page<T>, CodecError> {
    let epoch = LogEpoch(d.fixed()?);
    let after = u64::from_be_bytes(d.fixed()?);
    let through = u64::from_be_bytes(d.fixed()?);
    let high = u64::from_be_bytes(d.fixed()?);
    let count = d.count()?;
    if count > MAX_PAGE_ENTRIES {
        return Err(CodecError::TooManyItems);
    }
    let mut entries = Vec::new();
    let mut total = 0;
    for _ in 0..count {
        let seq = u64::from_be_bytes(d.fixed()?);
        let len = d.u32()? as usize;
        if len > MAX_RECORD_BYTES || len > MAX_PAGE_RECORD_BYTES - total {
            return Err(CodecError::TooLarge);
        }
        total += len;
        let record = parse(d.raw(len)?)?;
        entries.push(LogEntry { seq, record });
    }
    Ok(Page {
        epoch,
        after,
        through,
        high,
        entries,
    })
}
pub fn encode_peer_request(request: &PeerRequest) -> Result<Vec<u8>, CodecError> {
    let mut e = header(b"HPQ\0", request.cluster, request.node)?;
    match &request.command {
        PeerCommand::Status => e.tag(0)?,
        PeerCommand::Cursor { epoch } => {
            e.tag(1)?;
            e.raw(&epoch.0)?;
        }
        PeerCommand::Page { epoch, after, high } => {
            e.tag(2)?;
            e.raw(&epoch.0)?;
            e.raw(&after.to_be_bytes())?;
            e.raw(&high.to_be_bytes())?;
        }
        PeerCommand::Accept(page) => {
            e.tag(3)?;
            put_page(&mut e, page, Record::bytes)?;
        }
        PeerCommand::MembershipStatus => e.tag(4)?,
        PeerCommand::MembershipCursor { epoch } => {
            e.tag(5)?;
            e.raw(&epoch.0)?;
        }
        PeerCommand::MembershipPage { epoch, after, high } => {
            e.tag(6)?;
            e.raw(&epoch.0)?;
            e.raw(&after.to_be_bytes())?;
            e.raw(&high.to_be_bytes())?;
        }
        PeerCommand::MembershipAccept(page) => {
            e.tag(7)?;
            put_page(&mut e, page, crate::membership::Certificate::bytes)?;
        }
    }
    Ok(e.finish())
}
pub fn decode_peer_request(bytes: &[u8]) -> Result<PeerRequest, CodecError> {
    let (mut d, cluster, node) = parse_header(bytes, b"HPQ\0")?;
    let command = match d.tag()? {
        0 => PeerCommand::Status,
        1 => PeerCommand::Cursor {
            epoch: LogEpoch(d.fixed()?),
        },
        2 => PeerCommand::Page {
            epoch: LogEpoch(d.fixed()?),
            after: u64::from_be_bytes(d.fixed()?),
            high: u64::from_be_bytes(d.fixed()?),
        },
        3 => PeerCommand::Accept(get_page(&mut d, Record::from_bytes)?),
        4 => PeerCommand::MembershipStatus,
        5 => PeerCommand::MembershipCursor {
            epoch: LogEpoch(d.fixed()?),
        },
        6 => PeerCommand::MembershipPage {
            epoch: LogEpoch(d.fixed()?),
            after: u64::from_be_bytes(d.fixed()?),
            high: u64::from_be_bytes(d.fixed()?),
        },
        7 => PeerCommand::MembershipAccept(get_page(
            &mut d,
            crate::membership::Certificate::from_bytes,
        )?),
        _ => return Err(CodecError::InvalidTag),
    };
    d.finish()?;
    Ok(PeerRequest {
        cluster,
        node,
        command,
    })
}
pub fn encode_peer_response(response: &PeerResponse) -> Result<Vec<u8>, CodecError> {
    let mut e = header(b"HPS\0", response.cluster, response.node)?;
    match &response.reply {
        PeerReply::MembershipPage(page) => {
            e.tag(6)?;
            put_page(&mut e, page, crate::membership::Certificate::bytes)?;
        }
        PeerReply::Status { epoch, high } => {
            e.tag(0)?;
            e.raw(&epoch.0)?;
            e.raw(&high.to_be_bytes())?;
            for n in [
                1,
                MAX_RECORD_BYTES as u32,
                MAX_PAGE_RECORD_BYTES as u32,
                MAX_PAGE_ENTRIES as u32,
            ] {
                e.raw(&n.to_be_bytes())?;
            }
        }
        PeerReply::Cursor { epoch, through } | PeerReply::Ack { epoch, through } => {
            e.tag(if matches!(response.reply, PeerReply::Cursor { .. }) {
                1
            } else {
                3
            })?;
            e.raw(&epoch.0)?;
            e.raw(&through.to_be_bytes())?;
        }
        PeerReply::Page(page) => {
            e.tag(2)?;
            put_page(&mut e, page, Record::bytes)?;
        }
        PeerReply::CursorMismatch { epoch, current } => {
            e.tag(4)?;
            e.raw(&epoch.0)?;
            e.raw(&current.to_be_bytes())?;
        }
        PeerReply::Rejected { update_id, reason } => {
            e.tag(5)?;
            e.raw(&update_id.0)?;
            e.tag(match reason {
                Rejection::MissingDependency => 0,
                Rejection::IdMismatch => 1,
                Rejection::InvalidUpdate => 2,
            })?;
        }
    }
    Ok(e.finish())
}
pub fn decode_peer_response(bytes: &[u8]) -> Result<PeerResponse, CodecError> {
    let (mut d, cluster, node) = parse_header(bytes, b"HPS\0")?;
    let reply = match d.tag()? {
        0 => {
            let epoch = LogEpoch(d.fixed()?);
            let high = u64::from_be_bytes(d.fixed()?);
            let version = d.u32()?;
            if version != 1 {
                return Err(CodecError::UnsupportedVersion(version));
            }
            if d.u32()? != MAX_RECORD_BYTES as u32
                || d.u32()? != MAX_PAGE_RECORD_BYTES as u32
                || d.u32()? != MAX_PAGE_ENTRIES as u32
            {
                return Err(CodecError::NonCanonical);
            }
            PeerReply::Status { epoch, high }
        }
        1 => PeerReply::Cursor {
            epoch: LogEpoch(d.fixed()?),
            through: u64::from_be_bytes(d.fixed()?),
        },
        2 => PeerReply::Page(get_page(&mut d, Record::from_bytes)?),
        3 => PeerReply::Ack {
            epoch: LogEpoch(d.fixed()?),
            through: u64::from_be_bytes(d.fixed()?),
        },
        4 => PeerReply::CursorMismatch {
            epoch: LogEpoch(d.fixed()?),
            current: u64::from_be_bytes(d.fixed()?),
        },
        6 => PeerReply::MembershipPage(get_page(
            &mut d,
            crate::membership::Certificate::from_bytes,
        )?),
        5 => PeerReply::Rejected {
            update_id: UpdateId(d.fixed()?),
            reason: match d.tag()? {
                0 => Rejection::MissingDependency,
                1 => Rejection::IdMismatch,
                2 => Rejection::InvalidUpdate,
                _ => return Err(CodecError::InvalidTag),
            },
        },
        _ => return Err(CodecError::InvalidTag),
    };
    d.finish()?;
    Ok(PeerResponse {
        cluster,
        node,
        reply,
    })
}
