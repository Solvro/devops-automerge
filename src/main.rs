use std::env;

use cloneable_errors::{ErrorContext, ResContext};

use crate::{config::FileConfig, health::run_healthchecks, server::run_server};

mod config;
mod health;
mod routes;
mod server;
mod utils;

#[tokio::main]
async fn main() -> Result<(), ErrorContext> {
    tracing_subscriber::fmt::init();
    let config = FileConfig::get().context("Failed to read config")?;

    if env::args().nth(1).is_some_and(|x| x == "health") {
        run_healthchecks(config.listen).await
    } else {
        run_server(config).await
    }
}
