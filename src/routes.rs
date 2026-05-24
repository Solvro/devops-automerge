use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
};
use bytes::Bytes;
use cloneable_errors::ErrContext;
use octocrab::models::webhook_events::WebhookEvent;
use reqwest::StatusCode;
use tracing::{debug, error, warn};

use crate::{config::AppConfig, utils::verify_webhook_payload, webhook::process_webhook_event};

pub fn create_router(app_config: AppConfig) -> Router<()> {
    Router::new()
        .route("/", get(healthcheck))
        .route("/health", get(healthcheck))
        .route("/webhook", post(webhook))
        .with_state(app_config)
}

async fn healthcheck() -> impl IntoResponse {
    "elo żelo!!!"
}

async fn webhook(State(config): State<AppConfig>, headers: HeaderMap, body: Bytes) -> StatusCode {
    // validate the request
    if !verify_webhook_payload(&body, &headers, &config.webhook_secret) {
        return StatusCode::FORBIDDEN;
    }

    // parse
    let Some(event_type) = headers
        .get("X-GitHub-Event")
        .and_then(|h| h.to_str().ok())
        .map(ToOwned::to_owned)
    else {
        warn!("Invalid POST /webhook: no X-GitHub-Event header");
        return StatusCode::BAD_REQUEST;
    };
    let event = match WebhookEvent::try_from_header_and_body(&event_type, &body) {
        Ok(x) => Box::new(x),
        Err(e) => {
            warn!("Invalid POST /webhook: Failed to deserialize body: {e:?}");
            return StatusCode::BAD_REQUEST;
        }
    };
    debug!("Got {event_type} webhook event");

    // delegate processing to a new task
    tokio::spawn(async move {
        if let Err(e) = process_webhook_event(config, event).await {
            error!(
                "{:?}",
                e.context(format!("Error while processing webhook event {event_type}"))
            );
        }
    });

    // return 202
    StatusCode::ACCEPTED
}
