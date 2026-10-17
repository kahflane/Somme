use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::config::Config;
use crate::domain::CloudError;

const API_BASE: &str = "https://api.turso.tech/v1";

#[async_trait]
pub trait Provisioner: Send + Sync {
    async fn ensure_database(&self, name: &str) -> Result<String, CloudError>;
    async fn mint_token(&self, name: &str, ttl_days: u32) -> Result<String, CloudError>;
}

pub struct TursoPlatform {
    http: reqwest::Client,
    token: String,
    org: String,
    group: String,
}

#[derive(Deserialize)]
struct DatabaseEnvelope {
    database: DatabaseInfo,
}

#[derive(Deserialize)]
struct DatabaseInfo {
    #[serde(rename = "Hostname")]
    hostname: String,
}

#[derive(Deserialize)]
struct TokenEnvelope {
    jwt: String,
}

fn provisioning(err: impl std::fmt::Display) -> CloudError {
    CloudError::Provisioning(err.to_string())
}

impl TursoPlatform {
    pub fn new(config: &Config) -> Result<Self, CloudError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("somme-cloud")
                .build()
                .map_err(provisioning)?,
            token: config.turso_api_token.clone(),
            org: config.turso_org.clone(),
            group: config.turso_group.clone(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{API_BASE}/organizations/{}/{path}", self.org)
    }

    async fn get_database(&self, name: &str) -> Result<Option<String>, CloudError> {
        let response = self
            .http
            .get(self.url(&format!("databases/{name}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(provisioning)?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(
                response.json::<DatabaseEnvelope>().await.map_err(provisioning)?.database.hostname,
            )),
            status => Err(provisioning(format!("lookup returned {status}"))),
        }
    }
}

#[async_trait]
impl Provisioner for TursoPlatform {
    async fn ensure_database(&self, name: &str) -> Result<String, CloudError> {
        if let Some(host) = self.get_database(name).await? {
            return Ok(host);
        }
        let response = self
            .http
            .post(self.url("databases"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "name": name, "group": self.group }))
            .send()
            .await
            .map_err(provisioning)?;
        match response.status() {
            status if status.is_success() => Ok(response
                .json::<DatabaseEnvelope>()
                .await
                .map_err(provisioning)?
                .database
                .hostname),
            StatusCode::CONFLICT => self
                .get_database(name)
                .await?
                .ok_or_else(|| provisioning("database vanished after conflict")),
            status => Err(provisioning(format!("create returned {status}"))),
        }
    }

    async fn mint_token(&self, name: &str, ttl_days: u32) -> Result<String, CloudError> {
        let response = self
            .http
            .post(self.url(&format!(
                "databases/{name}/auth/tokens?expiration={ttl_days}d&authorization=full-access"
            )))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(provisioning)?;
        if !response.status().is_success() {
            return Err(provisioning(format!("token request returned {}", response.status())));
        }
        Ok(response.json::<TokenEnvelope>().await.map_err(provisioning)?.jwt)
    }
}
