//! Registration v4 (signed-HTTPS binding) through the native App API.
use harmion_client::Client;
use harmion_domain::*;
use harmion_net_native::{NativeTransport, SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{
    application,
    identity::Provenance,
    membership::PeerKey,
    membership_state::{self, MembershipSigner, Status},
    storage::{Identity, Store},
};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch, time::timeout};

fn key(n: u8) -> [u8; 32] {
    MembershipSigner::from_bytes(&[n; 32]).public_key()
}

#[tokio::test]
async fn registration_v4_is_validated_signed_and_owner_pinned() {
    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        epoch: LogEpoch([3; 32]),
    };
    let mut db = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
    let signer = Arc::new(MembershipSigner::from_bytes(&[2; 32]));
    membership_state::initialize(
        &mut db,
        &signer,
        harmion_protocol::membership::Member {
            node: identity.node,
            signing_key: signer.public_key(),
            binding: harmion_protocol::membership::Binding::Iroh(signer.public_key()),
            endpoint: String::new(),
        },
        true,
    )
    .unwrap();
    let store = SharedStore::new(db);
    let tls = Arc::new(TlsIdentity::generate().unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let transport = Arc::new(NativeTransport {
        membership_signer: Some(signer.clone()),
        tls: tls.clone(),
        iroh: None,
        signed_https_roots: Vec::new(),
    });
    let registration = PeerRegistration {
        version: 3,
        signing_key: Some(hex(&signer.public_key())),
        cluster_root: None,
        cluster_id: hex(&identity.cluster.0),
        node_id: hex(&identity.node.0),
        binding: PeerBinding::Https {
            certificate: hex(tls.certificate()),
            endpoint: endpoint.clone(),
        },
    };
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(app::serve_with_transport(
        listener,
        store.clone(),
        transport,
        registration,
        receiver,
    ));
    let info = ConnectionInfo {
        version: 1,
        cluster_id: hex(&identity.cluster.0),
        node_id: hex(&identity.node.0),
        certificate: hex(tls.certificate()),
        endpoint,
    };
    let client = Client::new(info, *admin.expose_bytes()).unwrap();

    let cloud = [9u8; 32];
    let cloud_key = key(9);
    let v4 = serde_json::json!({
        "version": 4,
        "cluster_id": hex(&identity.cluster.0),
        "node_id": hex(&cloud),
        "signing_key": hex(&cloud_key),
        "binding": {"transport": "signed_https", "origin": "https://replica.example"},
    });
    client
        .call("approve", &serde_json::to_vec(&v4).unwrap())
        .await
        .unwrap();

    store
        .run(move |s| {
            s.transaction(|tx| {
                let peer = tx.peer(NodeId(cloud))?.expect("routing projection");
                assert_eq!(peer.key, PeerKey::SignedHttps(cloud_key));
                assert_eq!(peer.endpoint, "https://replica.example");
                let pin = tx
                    .known_peer_identity(NodeId(cloud))?
                    .expect("owner registration pin");
                assert_eq!(pin.signing_key, cloud_key);
                assert_eq!(pin.key, PeerKey::SignedHttps(cloud_key));
                assert_eq!(pin.provenance, Provenance::OwnerRegistration);
                let state = membership_state::state(tx)?.unwrap();
                assert!(matches!(
                    state.nodes[&NodeId(cloud)],
                    Status::Active(harmion_protocol::membership::Member {
                        binding: harmion_protocol::membership::Binding::SignedHttps(key),
                        ..
                    }) if key == cloud_key
                ));
                Ok(())
            })
        })
        .await
        .unwrap();

    // Wrong version for the binding, missing signing key and non-canonical origin
    // are all rejected before any state changes.
    let mut v3 = v4.clone();
    v3["version"] = serde_json::json!(3);
    let mut missing = v4.clone();
    missing.as_object_mut().unwrap().remove("signing_key");
    let mut non_canonical = v4.clone();
    non_canonical["binding"]["origin"] = serde_json::json!("https://Replica.example");
    let mut ip_literal = v4.clone();
    ip_literal["binding"]["origin"] = serde_json::json!("https://127.0.0.1");
    for document in [v3, missing, non_canonical, ip_literal] {
        assert!(
            matches!(
                client
                    .call("approve", &serde_json::to_vec(&document).unwrap())
                    .await,
                Err(harmion_client::Error::Http(400))
            ),
            "{document}"
        );
    }
    store
        .run(move |s| {
            s.transaction(|tx| {
                assert_eq!(tx.peers()?.len(), 1, "rejections must not register peers");
                Ok(())
            })
        })
        .await
        .unwrap();

    let _ = stop.send(true);
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
