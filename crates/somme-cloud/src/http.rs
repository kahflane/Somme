use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderName, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};
use tower_governor::governor::GovernorConfigBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::domain::{CloudError, GitHubConfig, GitHubSignIn, GitHubToken, SessionView, SyncCredentials};
use crate::service::AccountService;

const BODY_LIMIT: usize = 16 * 1024;

type Shared = Arc<AccountService>;

impl IntoResponse for CloudError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::BadCredentials | Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Provisioning(_) | Self::GitHub(_) => StatusCode::BAD_GATEWAY,
        };
        let message = if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
            "the sync service is having trouble, try again soon".to_owned()
        } else {
            self.to_string()
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

fn bearer(headers: &HeaderMap) -> Result<&str, CloudError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
        .ok_or(CloudError::Unauthorized)
}

async fn github_config(State(svc): State<Shared>) -> Json<GitHubConfig> {
    Json(svc.github_config())
}

async fn sign_in_with_github(State(svc): State<Shared>, Json(req): Json<GitHubSignIn>) -> Result<Json<SessionView>, CloudError> {
    Ok(Json(svc.sign_in_with_github(&req).await?))
}

async fn github_token(
    State(svc): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<GitHubSignIn>,
) -> Result<Json<GitHubToken>, CloudError> {
    Ok(Json(svc.github_token(bearer(&headers)?, &req).await?))
}

async fn logout(State(svc): State<Shared>, headers: HeaderMap) -> Result<StatusCode, CloudError> {
    svc.logout(bearer(&headers)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn credentials(State(svc): State<Shared>, headers: HeaderMap) -> Result<Json<SyncCredentials>, CloudError> {
    Ok(Json(svc.credentials(bearer(&headers)?).await?))
}

async fn health() -> &'static str {
    "ok"
}

pub fn routes(svc: Shared) -> Router {
    Router::new()
        .route("/v1/auth/github", get(github_config))
        .route("/v1/sessions/github", post(sign_in_with_github))
        .route("/v1/sessions/current", delete(logout))
        .route("/v1/github/token", post(github_token))
        .route("/v1/sync/credentials", post(credentials))
        .with_state(svc)
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
}

/// Rate limits by client IP. Behind a proxy every peer is the proxy, so the IP
/// comes from the proxy's header when one is set.
#[derive(Clone)]
struct ClientIp(Option<HeaderName>);

impl KeyExtractor for ClientIp {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<IpAddr, GovernorError> {
        match &self.0 {
            Some(name) => req
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok()),
            None => req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.ip()),
        }
        .ok_or(GovernorError::UnableToExtractKey)
    }
}

pub fn router(svc: Shared, client_ip_header: Option<HeaderName>) -> Router {
    let limits = GovernorConfigBuilder::default()
        .key_extractor(ClientIp(client_ip_header))
        .per_second(2)
        .burst_size(10)
        .finish()
        .expect("rate limits are not zero");
    Router::new()
        .merge(routes(svc).layer(GovernorLayer::new(limits)))
        .route("/healthz", get(health))
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::service::tests::{service, sign_in};

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::post(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn full_flow_over_http() {
        let (_dir, svc, _) = service().await;
        let app = routes(Arc::new(svc));
        let (status, body) = call(&app, Request::get("/v1/auth/github").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clientId"], "fake-client");
        let good = sign_in("user-3");
        let body_for = |code: &str| {
            json!({"code": code, "redirectUri": good.redirect_uri, "codeVerifier": good.code_verifier})
        };
        let (status, body) = call(&app, post_json("/v1/sessions/github", body_for(&good.code))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["login"], "user3");
        assert!(body["avatarUrl"].as_str().unwrap().starts_with("https://"));
        let token = body["token"].as_str().unwrap().to_owned();
        let (status, _) = call(&app, post_json("/v1/sessions/github", body_for("forged"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let req = Request::post("/v1/sync/credentials")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(&app, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["url"].as_str().unwrap().starts_with("libsql://"));
        assert!(body["expiresAt"].as_i64().unwrap() > 0);
        let (status, _) = call(&app, Request::post("/v1/sync/credentials").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let mut req = post_json("/v1/github/token", body_for(&good.code));
        req.headers_mut().insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        let (status, body) = call(&app, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["token"], "gho_for_user3");
        assert_eq!(body["scope"], "repo");
        let (status, _) = call(&app, post_json("/v1/github/token", body_for(&good.code))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "needs a session");
    }

    #[test]
    fn rate_limit_key_trusts_the_proxy_header_only_when_set() {
        let req = Request::get("/").header("fly-client-ip", "203.0.113.7").body(()).unwrap();
        let behind_proxy = ClientIp(Some(HeaderName::from_static("fly-client-ip")));
        assert_eq!(behind_proxy.extract(&req).ok(), "203.0.113.7".parse().ok());
        assert!(ClientIp(None).extract(&req).is_err());
    }
}
