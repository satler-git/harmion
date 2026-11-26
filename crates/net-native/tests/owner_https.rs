use harmion_client::{Client, Error, SshSigner};
use harmion_domain::*;
use harmion_net_native::{SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{
    application, owner,
    storage::{Identity, Store},
};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use serde_json::json;
use std::{process::Command, sync::Arc};
use tokio::{net::TcpListener, sync::watch};

fn key(directory: &std::path::Path, name: &str) -> String {
    let path = directory.join(name);
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    std::fs::read_to_string(directory.join(format!("{name}.pub"))).unwrap()
}

#[tokio::test]
async fn owner_enrollment_requires_a_node_verified_assertion() {
    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        epoch: LogEpoch([3; 32]),
    };
    let mut db = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
    let keys = std::env::temp_dir().join(format!("harmion-owner-https-{}", std::process::id()));
    std::fs::create_dir_all(&keys).unwrap();
    let primary = key(&keys, "primary");
    db.transaction(|tx| owner::enroll_initial(tx, &primary, "primary", &[7; 32], [9; 32], 100))
        .unwrap();

    let store = SharedStore::new(db);
    let tls = Arc::new(TlsIdentity::generate().unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let info = ConnectionInfo {
        version: 1,
        cluster_id: hex(&identity.cluster.0),
        node_id: hex(&identity.node.0),
        certificate: hex(tls.certificate()),
        endpoint: endpoint.clone(),
        oauth_origin: None,
    };
    let registration = PeerRegistration {
        signing_key: None,
        cluster_root: None,
        version: 2,
        cluster_id: info.cluster_id.clone(),
        node_id: info.node_id.clone(),
        binding: PeerBinding::Https {
            certificate: info.certificate.clone(),
            endpoint,
        },
    };
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(app::serve(
        listener,
        store.clone(),
        tls,
        registration,
        receiver,
    ));
    let client = Client::new(info, *admin.expose_bytes()).unwrap();
    let status = client.owner_status().await.unwrap();
    assert_eq!(status["enforce"], true);
    assert_eq!(status["credentials"].as_array().unwrap().len(), 1);

    let backup = key(&keys, "backup");
    let body = serde_json::to_vec(&json!({"public_key": backup, "label": "backup"})).unwrap();
    assert!(matches!(
        client.call("owner-enroll", &body).await,
        Err(Error::Http(403))
    ));

    let signer = SshSigner::new(keys.join("primary.pub"));
    let enrolled = client
        .call_owner("owner-enroll", &body, None, None, &signer)
        .await
        .unwrap();
    assert_eq!(enrolled["label"], "backup");
    let status = client.owner_status().await.unwrap();
    assert_eq!(status["credentials"].as_array().unwrap().len(), 2);

    // A key that is not enrolled cannot authorize a second enrollment.
    let outsider = key(&keys, "outsider");
    let body = serde_json::to_vec(&json!({"public_key": outsider, "label": "outsider"})).unwrap();
    let outsider = SshSigner::new(keys.join("outsider.pub"));
    assert!(matches!(
        client
            .call_owner("owner-enroll", &body, None, None, &outsider)
            .await,
        Err(Error::Http(403))
    ));

    // Rotating the recovery code needs the owner assertion and invalidates the
    // code shown at enrollment.
    assert!(matches!(
        client.call("owner-rotate-recovery", &[]).await,
        Err(Error::Http(403))
    ));
    let rotated = client
        .call_owner("owner-rotate-recovery", &[], None, None, &signer)
        .await
        .unwrap();
    let new_code = rotated["recovery_code"].as_str().unwrap().to_owned();
    let recovered_key = key(&keys, "recovered");
    let old_body = serde_json::to_vec(
        &json!({"recovery_code": hex(&[7; 32]), "public_key": recovered_key.clone()}),
    )
    .unwrap();
    assert!(matches!(
        client.call("owner-recover", &old_body).await,
        Err(Error::Http(403))
    ));
    let new_body = serde_json::to_vec(
        &json!({"recovery_code": new_code.clone(), "public_key": recovered_key}),
    )
    .unwrap();
    let recovered = client.call("owner-recover", &new_body).await.unwrap();
    assert!(
        recovered["fingerprint"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(recovered["recovery_code"].as_str().is_some());

    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
    let _ = std::fs::remove_dir_all(keys);
}
