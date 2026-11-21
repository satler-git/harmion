//! Native sync loop over real signed-HTTPS TLS, using the same core the daemon
//! runs. The peer is a native v2 server here; the workerd DO peer is exercised
//! separately by design_checks/worker_do.
use harmion_domain::{ClusterId, NodeId};
use harmion_net_native::{NativeTransport, SharedStore, peer_v2, sync, tls::TlsIdentity};
use harmion_node::{
    https_peer::Config,
    membership_state::MembershipSigner,
    replication,
    storage::{Store, accept_records},
};
use harmion_protocol::{
    LogEpoch, PeerCommand, PeerReply, Record,
    membership::{Action, Binding, Certificate, Member, Statement},
};
use harmion_store_sqlite::SqliteStore;
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch, time::timeout};

const CLUSTER: ClusterId = ClusterId([1; 32]);
fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn member(n: u8, endpoint: String) -> Member {
    Member {
        node: node(n),
        signing_key: signer(n).public_key(),
        binding: Binding::SignedHttps(signer(n).public_key()),
        endpoint,
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

fn provision(
    store: &mut SqliteStore,
    node: u8,
    root: &Certificate,
    evidence: &[Certificate],
    own: Member,
) {
    use harmion_node::provisioning::{self, Manifest};
    provisioning::provision(
        store,
        &signer(node),
        &Manifest {
            own,
            root: root.clone(),
            evidence,
            contacts: &[],
        },
    )
    .unwrap();
}

#[tokio::test]
async fn native_sync_pushes_and_pulls_over_signed_https() {
    // Throwaway CA and `localhost` certificate for the v2 server.
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("https://localhost:{port}");
    let root = admit(2, member(2, origin.clone()));
    let evidence = [admit(2, member(3, "https://node3.example".to_owned()))];

    let mut server_db = SqliteStore::open(
        ":memory:",
        harmion_node::storage::Identity {
            cluster: CLUSTER,
            node: node(2),
            epoch: LogEpoch([2; 32]),
        },
    )
    .unwrap();
    provision(
        &mut server_db,
        2,
        &root,
        &evidence,
        member(2, origin.clone()),
    );
    let mut client_db = SqliteStore::open(
        ":memory:",
        harmion_node::storage::Identity {
            cluster: CLUSTER,
            node: node(3),
            epoch: LogEpoch([3; 32]),
        },
    )
    .unwrap();
    provision(
        &mut client_db,
        3,
        &root,
        &evidence,
        member(3, "https://node3.example".to_owned()),
    );
    client_db
        .transaction(|tx| accept_records(tx, &[fixture()]))
        .unwrap();

    let server_store = SharedStore::new(server_db);
    let client_store = SharedStore::new(client_db);
    let server_signer = Arc::new(signer(2));
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(peer_v2::serve(
        listener,
        Arc::new(TlsIdentity::from_der(leaf.der().to_vec(), leaf_key.serialize_der()).unwrap()),
        server_store.clone(),
        server_signer,
        Config {
            origin: origin.clone(),
        },
        receiver,
    ));

    let transport = NativeTransport {
        membership_signer: Some(Arc::new(signer(3))),
        tls: Arc::new(TlsIdentity::generate().unwrap()),
        iroh: None,
        signed_https_roots: vec![ca.der().to_vec()],
    };
    let identity = harmion_node::storage::Identity {
        cluster: CLUSTER,
        node: node(3),
        epoch: LogEpoch([3; 32]),
    };
    let report = timeout(
        Duration::from_secs(20),
        sync::sync_once(&client_store, &transport, node(2)),
    )
    .await
    .expect("sync timeout")
    .expect("sync");
    assert_eq!(report.uploaded, 1);
    assert_eq!(
        server_store
            .run(|s| s.transaction(replication::high))
            .await
            .unwrap(),
        1
    );
    println!("PASS: native sync pushed a record over signed-HTTPS TLS");

    // The peer now has it; the second round pulls the same durable record and
    // advances the client cursor without duplicating it.
    let report = timeout(
        Duration::from_secs(20),
        sync::sync_once(&client_store, &transport, node(2)),
    )
    .await
    .expect("sync timeout")
    .expect("sync");
    assert_eq!(report.downloaded, 1);
    let peer = harmion_node::membership::Peer {
        node: node(2),
        key: harmion_node::membership::PeerKey::SignedHttps(signer(2).public_key()),
        endpoint: origin.clone(),
        retired: false,
    };
    use harmion_net_native::sync::PeerTransport;
    let PeerReply::Status { high, .. } = transport
        .exchange(identity, &peer, PeerCommand::Status)
        .await
        .unwrap()
    else {
        panic!("status")
    };
    assert_eq!(high, 1);
    println!("PASS: second round pulled over signed-HTTPS TLS (high=1)");

    let _ = stop.send(true);
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
