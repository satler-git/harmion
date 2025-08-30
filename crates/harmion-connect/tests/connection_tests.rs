//! Async tests covering the (mpsc::Sender<T>, mpsc::Receiver<R>) usage pattern.
//! Frameworks used: Rust test + Tokio #[tokio::test].

use tokio::sync::mpsc;
use harmion_connect::Message;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

#[tokio::test]
async fn mpsc_tuple_send_and_recv_message() {
    let (tx_msg, mut rx_msg) = mpsc::channel::<Message>(8);
    let (tx_ack, mut rx_ack) = mpsc::channel::<bool>(8);

    // Prepare a message
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let msg = Message::new(b"hello".to_vec(), &key);

    // Sender side: send a message and await ack
    let send_handle = tokio::spawn({
        let mut tx_msg = tx_msg;
        let mut rx_ack = rx_ack;
        async move {
            tx_msg.send(msg).await.expect("send should succeed");
            // wait for ack
            let ack = rx_ack.recv().await.expect("expect an ack");
            assert!(ack);
        }
    });

    // Receiver side: receive message and send ack
    let recv_handle = tokio::spawn(async move {
        if let Some(received) = rx_msg.recv().await {
            assert!(received.verify(), "received message should verify");
            tx_ack.send(true).await.expect("ack send should succeed");
        } else {
            panic!("expected a message");
        }
    });

    let _ = tokio::join!(send_handle, recv_handle);
}

#[tokio::test]
async fn mpsc_tuple_recv_none_when_channel_closed() {
    let (_tx, mut rx) = mpsc::channel::<u32>(1);
    drop(_tx); // close sender
    let got = rx.recv().await;
    assert!(got.is_none(), "recv should return None when all senders are dropped");
}