use cloneable_errors::{ErrorContext, ResContext};
use tokio::net::TcpListener;
use tracing::info;

use crate::routes::create_router;

pub async fn run_server() -> Result<(), ErrorContext> {
    let router = create_router();
    let listener = TcpListener::bind("[::]:8080")
        .await
        .context("Failed to bind to TCP port 8080")?;
    info!("listening on [::]:8080");
    axum::serve(listener, router)
        .await
        .context("Error while serving on TCP 8080")
}
