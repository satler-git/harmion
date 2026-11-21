//! End-to-end signed-HTTPS v2 peer exchange over a real loopback TLS connection.
//!
//! The test CA is a test-only trust anchor; the client still performs a full
//! WebPKI chain, hostname, expiry and server-auth validation. No certificate
//! verification is disabled and no redirect is followed.
use harmion_domain::*;
use harmion_net_native::{NativeTransport, SharedStore, SystemHost, peer_v2, tls::TlsIdentity};
use harmion_node::{
    application,
    https_peer::Config,
    membership::Peer,
    membership_state::{self, MembershipSigner},
    storage::Identity,
};
use harmion_protocol::membership::{Binding, Member};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch, time::timeout};

fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}

#[tokio::test]
async fn signed_https_peer_v2_round_trip_over_webpki_loopback() {
    // A throwaway test CA and a `localhost` server certificate.
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();

    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: node(2),
        epoch: LogEpoch([2; 32]),
    };
    let server_signer = Arc::new(MembershipSigner::from_bytes(&[2; 32]));
    let client_signer = Arc::new(MembershipSigner::from_bytes(&[3; 32]));
    let mut db = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
    membership_state::initialize(
        &mut db,
        &server_signer,
        Member {
            node: node(2),
            signing_key: server_signer.public_key(),
            binding: Binding::SignedHttps(server_signer.public_key()),
            endpoint: String::new(),
        },
        true,
    )
    .unwrap();
    membership_state::approve(
        &mut db,
        0,
        &admin,
        &server_signer,
        ClusterId([1; 32]),
        Member {
            node: node(3),
            signing_key: client_signer.public_key(),
            binding: Binding::SignedHttps(client_signer.public_key()),
            endpoint: "https://client.example".to_owned(),
        },
        [9; 32],
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("https://localhost:{port}");
    let store = SharedStore::new(db);
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(peer_v2::serve(
        listener,
        Arc::new(TlsIdentity::from_der(leaf.der().to_vec(), leaf_key.serialize_der()).unwrap()),
        store.clone(),
        server_signer.clone(),
        Config {
            origin: origin.clone(),
        },
        receiver,
    ));
    let transport = NativeTransport {
        membership_signer: Some(client_signer.clone()),
        tls: Arc::new(TlsIdentity::generate().unwrap()),
        iroh: None,
        signed_https_roots: vec![ca.der().to_vec()],
    };
    let local = Identity {
        cluster: ClusterId([1; 32]),
        node: node(3),
        epoch: LogEpoch([3; 32]),
    };
    let peer = Peer {
        node: node(2),
        key: harmion_node::membership::PeerKey::SignedHttps(server_signer.public_key()),
        endpoint: origin.clone(),
        retired: false,
    };
    use harmion_net_native::sync::PeerTransport;
    let reply = transport
        .exchange(local, &peer, PeerCommand::MembershipStatus)
        .await
        .unwrap();
    assert!(matches!(reply, PeerReply::Status { .. }));
    // Data is served only because both nodes are currently Active.
    let reply = transport
        .exchange(local, &peer, PeerCommand::Status)
        .await
        .unwrap();
    assert!(matches!(reply, PeerReply::Status { .. }));

    // A different configured origin is a different identity binding.
    let wrong_origin = Peer {
        endpoint: "https://elsewhere.example".to_owned(),
        ..peer.clone()
    };
    assert!(
        transport
            .exchange(local, &wrong_origin, PeerCommand::Status)
            .await
            .is_err()
    );
    // A server proof signed by another key is rejected locally.
    let wrong_key = Peer {
        key: harmion_node::membership::PeerKey::SignedHttps([44; 32]),
        ..peer.clone()
    };
    assert!(
        transport
            .exchange(local, &wrong_key, PeerCommand::Status)
            .await
            .is_err()
    );
    // An unpinned requester is refused before any challenge is allocated.
    let stranger = NativeTransport {
        membership_signer: Some(Arc::new(MembershipSigner::from_bytes(&[9; 32]))),
        tls: Arc::new(TlsIdentity::generate().unwrap()),
        iroh: None,
        signed_https_roots: vec![ca.der().to_vec()],
    };
    assert!(
        stranger
            .exchange(local, &peer, PeerCommand::MembershipStatus)
            .await
            .is_err()
    );

    // A server certificate for another hostname fails full WebPKI validation;
    // there is no self-signed or `harmion.local` bypass in this transport.
    let other_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_port = other_listener.local_addr().unwrap().port();
    let other_key = rcgen::KeyPair::generate().unwrap();
    let mut other_params = rcgen::CertificateParams::new(vec!["other.example".to_owned()]).unwrap();
    other_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let other_leaf = other_params.signed_by(&other_key, &ca).unwrap();
    let (other_stop, other_receiver) = watch::channel(false);
    let other_server = tokio::spawn(peer_v2::serve(
        other_listener,
        Arc::new(
            TlsIdentity::from_der(other_leaf.der().to_vec(), other_key.serialize_der()).unwrap(),
        ),
        store.clone(),
        server_signer.clone(),
        Config {
            origin: format!("https://localhost:{other_port}"),
        },
        other_receiver,
    ));
    assert!(
        transport
            .exchange(
                local,
                &Peer {
                    endpoint: format!("https://localhost:{other_port}"),
                    ..peer.clone()
                },
                PeerCommand::MembershipStatus
            )
            .await
            .is_err()
    );
    let _ = other_stop.send(true);
    timeout(Duration::from_secs(5), other_server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let _ = stop.send(true);
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
