use cloneable_errors::ErrorContext;

use crate::server::run_server;

mod routes;
mod server;

#[tokio::main]
async fn main() -> Result<(), ErrorContext> {
    tracing_subscriber::fmt::init();

    run_server().await
}
