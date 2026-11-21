use std::{net::SocketAddr, sync::Arc, time::Duration};

use harmion_net_native::recovery::{self, Reason};
use iroh::{Endpoint, Watcher as _};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use tokio::{sync::watch, time::timeout};

const DEADLINE: Duration = Duration::from_secs(20);

async fn relay(address: SocketAddr) -> Server {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(address));
    Server::spawn(config).await.unwrap()
}

async fn connected(endpoint: &Endpoint, expected: bool) {
    let mut status = endpoint.home_relay_status();
    timeout(DEADLINE, async {
        loop {
            if status.get().iter().any(|relay| relay.is_connected()) == expected {
                break;
            }
            status.updated().await.unwrap();
        }
    })
    .await
    .expect("endpoint did not reach expected relay connection state");
}

async fn relay_hint(receiver: &mut watch::Receiver<Option<Reason>>) {
    timeout(DEADLINE, async {
        loop {
            receiver.changed().await.unwrap();
            if *receiver.borrow_and_update() == Some(Reason::RelayConnected) {
                break;
            }
        }
    })
    .await
    .expect("relay recovery did not produce a RelayConnected hint");
}

#[tokio::test]
async fn local_relay_restart_emits_recovery_hint() {
    let server = relay((std::net::Ipv4Addr::LOCALHOST, 0).into()).await;
    let address = server.http_addr().unwrap();
    let url: iroh::RelayUrl = format!("http://{address}").parse().unwrap();
    let endpoint = iroh::endpoint::Builder::empty()
        .crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .clear_ip_transports()
        .relay_mode(iroh::RelayMode::custom([url]))
        .bind()
        .await
        .unwrap();
    timeout(DEADLINE, endpoint.online()).await.unwrap();
    assert_eq!(endpoint.addr().ip_addrs().count(), 0);

    let (sender, mut receiver) = watch::channel(None);
    let monitor = tokio::spawn(recovery::monitor(sender, Some(endpoint.clone())));

    // There is no public monitor readiness acknowledgment. A priming down/up
    // cycle's actual RelayConnected event acknowledges the relay observer,
    // rather than assuming that yield_now has initialized its baseline.
    server.shutdown().await.unwrap();
    connected(&endpoint, false).await;
    // Allow the independent relay observer to consume the disconnected snapshot
    // before reconnecting; this is settling time, not a readiness assertion.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let server = relay(address).await;
    relay_hint(&mut receiver).await;
    connected(&endpoint, true).await;

    // Now the monitor is known to have observed a live relay. Drain all hints
    // before a second real shutdown/restart, so neither initial connection nor
    // the priming recovery can satisfy the assertion below.
    receiver.borrow_and_update();
    server.shutdown().await.unwrap();
    connected(&endpoint, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    receiver.borrow_and_update();
    let server = relay(address).await;
    relay_hint(&mut receiver).await;
    connected(&endpoint, true).await;

    drop(receiver);
    timeout(DEADLINE, monitor)
        .await
        .expect("monitor did not stop after its receiver disappeared")
        .unwrap();
    endpoint.close().await;
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn monitor_stops_when_receiver_is_dropped_without_an_endpoint() {
    let (sender, receiver) = watch::channel(None);
    let monitor = tokio::spawn(recovery::monitor(sender, None));
    drop(receiver);
    timeout(Duration::from_secs(2), monitor)
        .await
        .expect("monitor leaked after receiver drop")
        .unwrap();
}
