use axum::{Router, response::IntoResponse, routing::get};

pub fn create_router() -> Router<()> {
    Router::new()
        .route("/", get(healthcheck))
        .route("/health", get(healthcheck))
}

async fn healthcheck() -> impl IntoResponse {
    "elo żelo!!!"
}
