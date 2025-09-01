pub(super) mod client;
pub(super) mod server;

use crate::PeerIndex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct InitClientToSig {
    known_signals: Vec<SignalInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InitSigToClient {
    known_signals: Vec<SignalInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone, strum::EnumIs)]
pub(super) enum SignalData {
    Sdp(Box<webrtc::peer_connection::sdp::session_description::RTCSessionDescription>),
    Ice(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit),
}

impl PartialEq for SignalData {
    fn eq(&self, rhs: &Self) -> bool {
        match (self, rhs) {
            (SignalData::Sdp(lhs), SignalData::Sdp(rhs)) => lhs.sdp == rhs.sdp,
            (SignalData::Ice(lhs), SignalData::Ice(rhs)) => lhs == rhs,

            (SignalData::Sdp(_), SignalData::Ice(_)) | (SignalData::Ice(_), SignalData::Sdp(_)) => {
                false
            }
        }
    }
}

impl Eq for SignalData {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct SignalMessage {
    pub origin: PeerIndex,
    pub to: PeerIndex,

    pub data: SignalData,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct SignalInfo {
    addr: (String, u16), // ip or domain(w/tailscale?)
    id: PeerIndex,
}

impl SignalInfo {
    fn to_http(&self) -> String {
        format!("http://{}:{}", self.addr.0, self.addr.1)
    }

    fn to_ws(&self) -> String {
        format!("ws://{}:{}", self.addr.0, self.addr.1)
    }
}

#[cfg(test)]
mod tests {
    use tracing::info;

    use crate::{
        webrtc::{
            signal::{client::SignalClient, server::Signal, SignalData, SignalMessage},
            simple::Peer,
            BUFFER_SIZE,
        },
        Message, PeerIndex,
    };

    fn gen_id() -> PeerIndex {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;

        (&ed25519_dalek::SigningKey::generate(&mut rng)).into()
    }

    fn signal() -> Signal {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut rng);

        Signal::new(0, (&key).into(), key)
    }

    fn client() -> SignalClient {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut rng);

        SignalClient::new((&key).into(), key)
    }

    async fn sleep() {
        use tokio::time;

        time::sleep(time::Duration::from_millis(100)).await;
    }

    async fn wait_for_signal_count(
        peer: &mut SignalClient,
        want: usize,
        timeout_secs: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::time::{sleep, Duration, Instant};

        let start = Instant::now();
        loop {
            if peer.signal_count() == want {
                return Ok(());
            }
            if start.elapsed().as_secs() > timeout_secs {
                return Err(format!(
                    "wait_for_signal_count timed out (want: {}, got: {})",
                    want,
                    peer.signal_count()
                )
                .into());
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    use crate::Connection;

    // signal
    // - a
    // - b
    // peer
    // - 1
    //  - know a, b; will connect a
    // - 2
    //  - know a; will connect a
    // - 3
    //  - know b; will connect b

    #[tokio::test]
    async fn integrated_signal() -> Result<(), Box<dyn std::error::Error>> {
        crate::tests::init_log();

        let mut sig_a = signal();
        let mut sig_b = signal();

        sig_a.run("127.0.0.1").await?;
        sig_b.run("127.0.0.1").await?;

        let mut peer_1 = client();
        let mut peer_2 = client();
        let mut peer_3 = client();

        peer_1.insert_signal(sig_a.info().unwrap());
        peer_1.insert_signal(sig_b.info().unwrap());

        peer_2.insert_signal(sig_a.info().unwrap());

        peer_3.insert_signal(sig_b.info().unwrap());

        assert_eq!(peer_1.signal_count(), 2);
        assert_eq!(peer_2.signal_count(), 1);
        assert_eq!(peer_3.signal_count(), 1);

        peer_1.connect(sig_a.info()).await?;

        sleep().await;

        peer_2.connect(None).await?;

        sleep().await;

        peer_3.connect(None).await?;

        sleep().await;

        assert_eq!(peer_1.signal_count(), 2);

        wait_for_signal_count(&mut peer_2, 2, 5).await?;
        wait_for_signal_count(&mut peer_3, 2, 5).await?;

        assert_eq!(peer_2.signal_count(), 2);
        assert_eq!(peer_3.signal_count(), 2);

        let data: SignalData = SignalData::Ice(Default::default());

        let to_2 = SignalMessage {
            origin: peer_1.origin(),
            to: peer_2.origin(),
            data: data.clone(),
        };

        // exchange
        peer_1.send(to_2.clone()).await?;

        assert_eq!(peer_2.recv().await?.unwrap().content, to_2);

        let to_3 = SignalMessage {
            origin: peer_1.origin(),
            to: peer_3.origin(),
            data: data.clone(),
        };

        // exchange
        peer_1.send(to_3.clone()).await?;

        assert_eq!(peer_3.recv().await?.unwrap().content, to_3);

        let to_2 = SignalMessage {
            origin: peer_3.origin(),
            to: peer_2.origin(),
            data: data.clone(),
        };

        // exchange
        peer_3.send(to_2.clone()).await?;

        assert_eq!(peer_2.recv().await?.unwrap().content, to_2);

        peer_1.disconnect();
        peer_2.disconnect();
        peer_3.disconnect();

        sig_a.shutdown();
        sig_b.shutdown();

        Ok(())
    }

    #[tokio::test]
    async fn peer_to_peer() -> Result<(), Box<dyn std::error::Error>> {
        crate::tests::init_log();

        let mut sig = signal();

        sig.run("127.0.0.1").await?;

        info!("the Signalling service is running");

        let mut peer_1_client = client();
        let mut peer_2_client = client();

        peer_1_client.connect(sig.info()).await?;

        sleep().await;

        peer_2_client.connect(sig.info()).await?;

        sleep().await;

        info!("peers are connected to the signal");

        use tokio::sync::mpsc;

        let peer_1_origin = peer_1_client.origin();
        let peer_2_origin = peer_2_client.origin();

        let peer_1_alias = crate::webrtc::simple::PeerAlias::from_key(peer_1_origin.0);
        let peer_2_alias = crate::webrtc::simple::PeerAlias::from_key(peer_2_origin.0);

        // 本来、このClientでゴニョゴニョするのはPeerPoolの責任
        let (sender, mut rx) = mpsc::channel(super::super::BUFFER_SIZE);
        let (tx, receiver) = mpsc::channel(super::super::BUFFER_SIZE);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    ice = rx.recv() => {
                        let Some(ice) = ice else { break };

                        let message = SignalMessage {
                            origin: peer_1_origin,
                            to: peer_2_origin,
                            data: SignalData::Ice(ice)
                        };

                        let _ = peer_1_client.send(message).await;
                    }
                    Ok(Some(msg)) = peer_1_client.recv() => {
                        let SignalData::Ice(ice) = msg.content.data else { continue };
                        let _ = tx.send(ice).await;
                    }
                }
            }
        });

        let (peer_1, sdp) = Peer::<crate::webrtc::simple::WaitingAnswer>::new(
            crate::webrtc::simple::Config {
                signal: Some((sender, receiver)),
                ..Default::default()
            },
            &peer_1_alias,
        )
        .await?;

        info!("spawned peer1's SignalClient Handler");

        let (sender, mut rx) = mpsc::channel(BUFFER_SIZE);
        let (tx, receiver) = mpsc::channel(BUFFER_SIZE);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    ice = rx.recv() => {
                        let Some(ice) = ice else { break };

                        let message = SignalMessage {
                            origin: peer_2_origin,
                            to: peer_1_origin,
                            data: SignalData::Ice(ice)
                        };

                        let _ = peer_2_client.send(message).await;
                    }
                    Ok(Some(msg)) = peer_2_client.recv() => {
                        let SignalData::Ice(ice) = msg.content.data else { continue };
                        let _ = tx.send(ice).await;
                    }
                }
            }
        });

        info!("spawned peer2's SignalClient Handler");

        let (peer_2, answer_sdp) = Peer::<crate::webrtc::simple::WaitingICE>::from_offer(
            sdp,
            crate::webrtc::simple::Config {
                signal: Some((sender, receiver)),
                ..Default::default()
            },
            &peer_2_alias,
        )
        .await?;

        let peer_1 = peer_1.set_remote_answer(answer_sdp).await?;
        let mut peer_2 = peer_2.wait().await?;

        let payload = "hello from A".to_string();

        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;

        peer_1
            .send(&Message::new(
                payload.clone().into(),
                &ed25519_dalek::SigningKey::generate(&mut rng),
            ))
            .await?;

        let received = tokio::time::timeout(std::time::Duration::from_secs(100), peer_2.recv())
            .await??
            .unwrap();

        assert!(received.verify());

        assert_eq!(String::from_utf8(received.content).unwrap(), payload);

        Ok(())
    }
}
