//! Test fixture only: loopback relay, stopped by closing stdin. Not a deployment configuration.
use std::io::{Read, Write};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new((
        std::net::Ipv4Addr::LOCALHOST,
        0,
    )));
    let relay = iroh_relay::server::Server::spawn(config).await?;
    println!(
        "http://{}",
        relay.http_addr().ok_or("missing relay address")?
    );
    std::io::stdout().flush()?;
    tokio::task::spawn_blocking(|| {
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input)
    })
    .await??;
    relay.shutdown().await?;
    Ok(())
}
