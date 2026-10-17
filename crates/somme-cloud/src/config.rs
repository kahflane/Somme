use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use axum::http::HeaderName;

const DEFAULT_ADDR: &str = "127.0.0.1:8787";
const DEFAULT_GROUP: &str = "default";
const DEFAULT_TOKEN_TTL_DAYS: u32 = 7;

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub data_dir: PathBuf,
    pub turso_api_token: String,
    pub turso_org: String,
    pub turso_group: String,
    pub token_ttl_days: u32,
    /// Header holding the real client IP when running behind a proxy (Fly sets
    /// `Fly-Client-IP`). Only set it when a proxy overwrites the header, or
    /// clients could fake it to dodge rate limits.
    pub client_ip_header: Option<HeaderName>,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            addr: var("SOMME_CLOUD_ADDR")
                .unwrap_or_else(|| DEFAULT_ADDR.into())
                .parse()
                .context("SOMME_CLOUD_ADDR is not a socket address")?,
            data_dir: var("SOMME_CLOUD_DATA").map_or_else(|| PathBuf::from("cloud-data"), PathBuf::from),
            turso_api_token: var("TURSO_API").context("TURSO_API is required")?,
            turso_org: var("TURSO_ORG").context("TURSO_ORG is required")?,
            turso_group: var("TURSO_GROUP").unwrap_or_else(|| DEFAULT_GROUP.into()),
            token_ttl_days: var("SOMME_TOKEN_TTL_DAYS")
                .map(|v| v.parse())
                .transpose()
                .context("SOMME_TOKEN_TTL_DAYS must be a number")?
                .unwrap_or(DEFAULT_TOKEN_TTL_DAYS),
            client_ip_header: var("SOMME_CLOUD_CLIENT_IP_HEADER")
                .map(|v| v.parse())
                .transpose()
                .context("SOMME_CLOUD_CLIENT_IP_HEADER is not a header name")?,
        })
    }
}
