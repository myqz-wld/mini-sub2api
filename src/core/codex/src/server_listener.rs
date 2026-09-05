use anyhow::{Context, Result};
use std::net::SocketAddr;

pub fn parse_internal_listen(raw: &str) -> Result<SocketAddr> {
    let address: SocketAddr = raw.parse().context("parsing internal listen address")?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "internal listener must be loopback"
    );
    Ok(address)
}
