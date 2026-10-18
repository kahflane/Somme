//! Trades the one-time code GitHub gives the app for the GitHub user. Only the
//! server holds the client secret, so the app cannot do this itself.

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::{CloudError, GitHubUser};

const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const API_BASE: &str = "https://api.github.com";

#[async_trait]
pub trait GitHub: Send + Sync {
    fn client_id(&self) -> &str;
    async fn user(&self, code: &str, redirect_uri: &str, code_verifier: &str) -> Result<GitHubUser, CloudError>;
}

pub struct GitHubApp {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
}

fn github(err: impl std::fmt::Display) -> CloudError {
    CloudError::GitHub(err.to_string())
}

impl GitHubApp {
    pub fn new(client_id: String, client_secret: String) -> Result<Self, CloudError> {
        Ok(Self {
            http: reqwest::Client::builder().user_agent("somme-cloud").build().map_err(github)?,
            client_id,
            client_secret,
        })
    }

    /// Somme only needs who the user is, so the GitHub token is thrown away
    /// once it has been used.
    async fn revoke(&self, token: &str) {
        let result = self
            .http
            .delete(format!("{API_BASE}/applications/{}/token", self.client_id))
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Accept", "application/vnd.github+json")
            .json(&serde_json::json!({ "access_token": token }))
            .send()
            .await;
        if let Err(err) = result.and_then(reqwest::Response::error_for_status) {
            tracing::warn!(%err, "could not revoke the GitHub token");
        }
    }
}

#[async_trait]
impl GitHub for GitHubApp {
    fn client_id(&self) -> &str {
        &self.client_id
    }

    async fn user(&self, code: &str, redirect_uri: &str, code_verifier: &str) -> Result<GitHubUser, CloudError> {
        let reply: TokenReply = self
            .http
            .post(TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", code_verifier),
            ])
            .send()
            .await
            .map_err(github)?
            .error_for_status()
            .map_err(github)?
            .json()
            .await
            .map_err(github)?;
        let Some(token) = reply.access_token else {
            tracing::info!(error = reply.error.as_deref().unwrap_or("none"), "GitHub refused the code");
            return Err(CloudError::BadCredentials);
        };
        let user = self
            .http
            .get(format!("{API_BASE}/user"))
            .bearer_auth(&token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(github);
        let user = match user {
            Ok(response) => response.json::<GitHubUser>().await.map_err(github),
            Err(err) => Err(err),
        };
        self.revoke(&token).await;
        user
    }
}
