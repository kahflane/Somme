use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::domain::{AuthRequest, CloudError, SessionView, SyncCredentials};
use crate::service::AccountService;

const BODY_LIMIT: usize = 16 * 1024;

type Shared = Arc<AccountService>;

impl IntoResponse for CloudError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::EmailTaken => StatusCode::CONFLICT,
            Self::BadCredentials | Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Provisioning(_) => StatusCode::BAD_GATEWAY,
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

async fn register(State(svc): State<Shared>, Json(req): Json<AuthRequest>) -> Result<(StatusCode, Json<SessionView>), CloudError> {
    Ok((StatusCode::CREATED, Json(svc.register(&req.email, &req.password).await?)))
}

async fn login(State(svc): State<Shared>, Json(req): Json<AuthRequest>) -> Result<Json<SessionView>, CloudError> {
    Ok(Json(svc.login(&req.email, &req.password).await?))
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
        .route("/v1/accounts", post(register))
        .route("/v1/sessions", post(login))
        .route("/v1/sessions/current", delete(logout))
        .route("/v1/sync/credentials", post(credentials))
        .with_state(svc)
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
}

pub fn router(svc: Shared) -> Router {
    let limits = GovernorConfigBuilder::default()
        .per_second(2)
        .burst_size(10)
        .finish()
        .unwrap_or_default();
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
    use crate::service::tests::service;

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
        let (status, body) = call(&app, post_json("/v1/accounts", json!({"email": "a@b.co", "password": "long enough pw"}))).await;
        assert_eq!(status, StatusCode::CREATED);
        let token = body["token"].as_str().unwrap().to_owned();
        let (status, _) = call(&app, post_json("/v1/sessions", json!({"email": "a@b.co", "password": "nope nope nope"}))).await;
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
    }
}
