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

#[derive(Debug, Serialize, Deserialize, Clone)]
enum SignalData {
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
    origin: PeerIndex,
    to: PeerIndex,

    data: SignalData,
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

    use crate::webrtc::signal::client::SignalClient;
    use crate::webrtc::signal::server::Signal;
    use crate::webrtc::signal::{SignalData, SignalMessage};
    use crate::PeerIndex;

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

    fn init_log() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            // .with_max_level(tracing::Level::ERROR)
            .with_file(true)
            .with_line_number(true)
            .init();
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

    #[tokio::test]
    async fn integrated_signal() -> Result<(), Box<dyn std::error::Error>> {
        // TODO: flaky
        init_log();

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

        assert_eq!(peer_2.recv().await?.content, to_2);

        let to_3 = SignalMessage {
            origin: peer_1.origin(),
            to: peer_3.origin(),
            data: data.clone(),
        };

        // exchange
        peer_1.send(to_3.clone()).await?;

        assert_eq!(peer_3.recv().await?.content, to_3);

        let to_2 = SignalMessage {
            origin: peer_3.origin(),
            to: peer_2.origin(),
            data: data.clone(),
        };

        // exchange
        peer_3.send(to_2.clone()).await?;

        assert_eq!(peer_2.recv().await?.content, to_2);

        peer_1.disconnect();
        peer_2.disconnect();
        peer_3.disconnect();

        sig_a.shutdown();
        sig_b.shutdown();

        Ok(())
    }
}
