use cloneable_errors::{ErrorContext, ResContext};

use crate::{config::FileConfig, server::run_server};

mod config;
mod routes;
mod server;

#[tokio::main]
async fn main() -> Result<(), ErrorContext> {
    tracing_subscriber::fmt::init();
    let config = FileConfig::get().context("Failed to read config")?;

    run_server(config).await
}
