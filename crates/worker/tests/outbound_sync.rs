//! Worker-initiated sync against a scripted peer transport.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    membership::Peer,
    membership_state::{self, MembershipSigner},
    provisioning::{self, Manifest},
    replication,
    storage::{Identity, Store},
};
use harmion_protocol::{
    LogEntry, LogEpoch, Page, PeerCommand, PeerReply, Record,
    membership::{Action, Binding, Certificate, Member, Statement},
};
use harmion_store_sqlite::SqliteStore;
use harmion_worker::outbound::{OutboundError, Transport, sync};
use std::cell::RefCell;

const CLUSTER: ClusterId = ClusterId([1; 32]);
fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn member(n: u8) -> Member {
    let key = signer(n).public_key();
    Member {
        node: node(n),
        signing_key: key,
        binding: Binding::SignedHttps(key),
        endpoint: format!("https://node{n}.example"),
    }
}
fn admit(issuer: u8, target: Member) -> Certificate {
    signer(issuer)
        .sign(Statement {
            cluster: CLUSTER,
            issuer: node(issuer),
            signing_key: signer(issuer).public_key(),
            nonce: [target.node.0[0]; 32],
            action: Action::Admit(target),
        })
        .unwrap()
}
fn identity() -> Identity {
    Identity {
        cluster: CLUSTER,
        node: node(3),
        epoch: LogEpoch([3; 32]),
    }
}
fn fixture() -> Record {
    let hex = include_str!("../../../contracts/fixtures/create-v1.hex").trim();
    let bytes: Vec<u8> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    Record::from_bytes(&bytes).unwrap()
}

struct Scripted {
    peer: Peer,
    commands: RefCell<Vec<String>>,
}
impl Transport for Scripted {
    async fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, OutboundError> {
        assert_eq!(peer.node, self.peer.node);
        self.commands.borrow_mut().push(format!("{command:?}"));
        Ok(match command {
            PeerCommand::MembershipStatus => PeerReply::Status {
                epoch: local.epoch,
                high: 0,
            },
            PeerCommand::MembershipCursor { epoch } => PeerReply::Cursor { epoch, through: 0 },
            PeerCommand::MembershipAccept(page) => PeerReply::Ack {
                epoch: page.epoch,
                through: page.through,
            },
            PeerCommand::Status => PeerReply::Status {
                epoch: local.epoch,
                high: 1,
            },
            PeerCommand::Cursor { epoch } => PeerReply::Cursor { epoch, through: 0 },
            PeerCommand::Page { epoch, after, high } => PeerReply::Page(Page {
                epoch,
                after,
                through: 1,
                high,
                entries: vec![LogEntry {
                    seq: 1,
                    record: fixture(),
                }],
            }),
            PeerCommand::Accept(page) => PeerReply::Ack {
                epoch: page.epoch,
                through: page.through,
            },
            _ => return Err(OutboundError::Invalid("unexpected command".into())),
        })
    }
}

#[tokio::test]
async fn outbound_pulls_records_and_pushes_membership() {
    let path = std::env::temp_dir().join(format!("harmion-outbound-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = SqliteStore::open(path.join("db"), identity()).unwrap();
    let root = admit(2, member(2));
    let evidence = [admit(2, member(3))];
    provisioning::provision(
        &mut store,
        &signer(3),
        &Manifest {
            own: member(3),
            root,
            evidence: &evidence,
            contacts: &[member(2)],
        },
    )
    .unwrap();
    let peer = store
        .transaction(|tx| tx.peer(node(2)))
        .unwrap()
        .expect("peer");
    let transport = Scripted {
        peer: peer.clone(),
        commands: RefCell::new(Vec::new()),
    };
    let report = sync(&mut store, &transport, identity(), peer)
        .await
        .unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(report.local_high, 0);
    assert_eq!(report.remote_high, 1);
    assert!(report.membership_uploaded > 0);
    store
        .transaction(|tx| {
            assert_eq!(replication::high(tx)?, 1);
            assert_eq!(tx.cursor(node(2), identity().epoch)?, 1);
            let page = replication::page(tx, identity().epoch, 0, 1)?;
            assert_eq!(page.entries.len(), 1);
            assert_eq!(page.entries[0].record.bytes(), fixture().bytes());
            Ok(())
        })
        .unwrap();
    store
        .transaction(|tx| {
            let state = membership_state::state(tx)?.unwrap();
            assert!(matches!(
                state.nodes[&node(3)],
                membership_state::Status::Active(_)
            ));
            Ok(())
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&path);
}
