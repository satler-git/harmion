use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use warp::Filter;

use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum SignalError {
    #[error("the Signalling Server is already running")]
    ServerRunning,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

type SigResult<T> = Result<T, SignalError>;

#[derive(Debug, Clone)]
pub(super) struct SignalInfo {
    addr: (String, u16), // ip or domain(w/tailscale?)
}

#[derive(Debug, Clone)]
pub(super) struct Signal {
    port: u16, // sig.port /= info.port の場合あり
    info: Option<ServerInfo>,
}

#[derive(Debug, Clone)]
struct ServerInfo {
    shutdown_token: CancellationToken,
    signal_info: SignalInfo,
}

impl Signal {
    pub(super) fn new(port: u16) -> Self {
        Self { port, info: None }
    }

    pub(super) async fn run(&mut self, host: &str) -> SigResult<()> {
        if self.info.is_some() {
            return Err(SignalError::ServerRunning);
        }

        let listener =
            tokio::net::TcpListener::bind::<std::net::SocketAddr>(([0, 0, 0, 0], self.port).into())
                .await?;

        let port = listener.local_addr()?.port();

        let token = CancellationToken::new();

        let token_c = token.clone();
        let srv = warp::serve(route())
            .incoming(listener)
            .graceful(async move {
                token_c.cancelled().await;
            });

        tokio::spawn(srv.run());

        self.info = Some(ServerInfo {
            shutdown_token: token,
            signal_info: SignalInfo {
                addr: (host.into(), port),
            },
        });

        Ok(())
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(c) = self.info.take() {
            c.shutdown_token.cancel()
        }
    }

    pub(super) fn info(&self) -> Option<SignalInfo> {
        self.info.as_ref().map(|c| c.signal_info.clone())
    }
}

fn route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let heartbeat = warp::path("heartbeat").and(heartbeat(Instant::now()));

    #[allow(clippy::let_and_return)]
    heartbeat // 将来的にorをつなげる
}

fn heartbeat(on: Instant) -> impl Filter<Extract = (String,), Error = warp::Rejection> + Clone {
    fn format_duration_human(dur: Duration) -> String {
        let mut r = String::new();
        let total_secs = dur.as_secs();

        let hours = total_secs / 3600;
        if hours > 0 {
            r += &format!("{}h", hours);
        }

        let minutes = (total_secs % 3600) / 60;
        if minutes > 0 {
            r += &format!("{}m", minutes);
        }

        let seconds = total_secs % 60;
        if seconds > 0 {
            r += &format!("{}s", seconds);
        }

        let millis = dur.subsec_millis();
        if millis > 0 {
            r += &format!("{}ms", millis);
        }

        if r.is_empty() {
            "0s".to_string()
        } else {
            r
        }
    }

    warp::get().map(move || format_duration_human(Instant::now().saturating_duration_since(on)))
}

#[cfg(test)]
mod tests {
    use warp::test::request;

    use crate::webrtc::signal::route;

    #[tokio::test]
    async fn heartbeat() -> Result<(), Box<dyn std::error::Error>> {
        let mut sig = super::Signal::new(5728);

        println!("starting the server");

        sig.run("test").await?;

        let client = reqwest::Client::new();

        println!("first request");
        let response = client.get("http://0.0.0.0:5728/heartbeat").send().await?;

        let body = response.text().await?;
        dbg!(body);

        println!("shutting down the server");
        sig.shutdown();

        println!("second request");
        let response = client.get("http://0.0.0.0:5728/heartbeat").send().await;
        assert!(response.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn heartbeat_warp() -> Result<(), Box<dyn std::error::Error>> {
        let route = route();

        assert!(request().path("/heartbeat").filter(&route).await.is_ok());

        Ok(())
    }
}
