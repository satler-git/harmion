//! Loopback-only wake endpoint used by the Cloudflare Worker alarm.
//!
//! The Worker cannot open a connection to a NATed native node. A Cloudflare
//! Tunnel forwards `POST /wake` here, and a successful bearer check asks the
//! automatic sync loop to run a round immediately. This endpoint carries no
//! data and grants no node permission beyond scheduling a sync attempt.
use harmion_local::ProtectedDir;
use harmion_protocol::unhex;
use std::{io, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
    time::timeout,
};

pub const TOKEN_FILE: &str = "wake.token";
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub fn load_token(directory: &ProtectedDir) -> io::Result<[u8; 32]> {
    let bytes = directory.read(TOKEN_FILE, 128)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "wake token is not UTF-8"))?;
    unhex::<32>(text.trim()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "wake token must be 64 hex characters",
        )
    })
}

pub async fn serve(listener: TcpListener, token: [u8; 32], wake: Arc<Notify>) -> io::Result<()> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let wake = wake.clone();
        tokio::spawn(async move {
            let _ = handle(&mut stream, &token, wake).await;
        });
    }
}

async fn handle(stream: &mut TcpStream, token: &[u8; 32], wake: Arc<Notify>) -> io::Result<()> {
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = timeout(READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "wake request timed out"))??;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.len() > MAX_REQUEST_BYTES {
            return respond(stream, 413).await;
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let text = match std::str::from_utf8(&request) {
        Ok(text) => text,
        Err(_) => return respond(stream, 400).await,
    };
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    if request_line.next() != Some("POST") || request_line.next() != Some("/wake") {
        return respond(stream, 403).await;
    }
    let authorization = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim())
    });
    let provided = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|value| unhex::<32>(value).ok());
    let Some(provided) = provided else {
        return respond(stream, 401).await;
    };
    if !bool::from(provided.ct_eq(token)) {
        return respond(stream, 401).await;
    }
    wake.notify_one();
    respond(stream, 204).await
}

async fn respond(stream: &mut TcpStream, status: u16) -> io::Result<()> {
    let reason = match status {
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let body = if status == 204 { "" } else { reason };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    async fn request(port: u16, authorization: Option<&str>) -> u16 {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let auth = authorization
            .map(|value| format!("Authorization: {value}\r\n"))
            .unwrap_or_default();
        let request =
            format!("POST /wake HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Length: 0\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response)
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn wake_requires_post_and_the_exact_bearer_token() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = [7u8; 32];
        let wake = Arc::new(Notify::new());
        tokio::spawn(serve(listener, token, wake));

        let good = format!("Bearer {}", harmion_protocol::hex(&token));
        assert_eq!(request(port, Some(&good)).await, 204);
        assert_eq!(
            request(
                port,
                Some(&format!("Bearer {}", harmion_protocol::hex(&[8u8; 32])))
            )
            .await,
            401
        );
        assert_eq!(request(port, None).await, 401);
    }
}
