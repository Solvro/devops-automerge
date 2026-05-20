use std::time::{Duration, Instant};

use cloneable_errors::{ErrorContext, ResContext, bail};
use reqwest::{ClientBuilder, StatusCode};
use tracing::info;

use crate::config::ListenConfig;

pub async fn run_healthchecks(listen_config: ListenConfig) -> Result<(), ErrorContext> {
    info!("Running in healthcheck mode");

    if let Some(unix_path) = listen_config.unix {
        let start = Instant::now();
        info!("Checking {unix_path} over unix sockets");
        let client = ClientBuilder::new()
            .unix_socket(&*unix_path)
            .timeout(Duration::from_secs(1))
            .build()
            .context("Failed to build new reqwest client")?;

        let response = client
            .get("http://localhost/health")
            .send()
            .await
            .with_context(|| format!("/health request over unix socket at {unix_path} failed"))?;
        let status = response.status();

        if status != StatusCode::OK {
            bail!("Expected /health to return code 200, got {status}",)
        }

        info!("Got 200 response in {}ms", start.elapsed().as_millis());
    }
    if let Some(target) = listen_config.tcp {
        let start = Instant::now();
        info!("Checking {target} over TCP");
        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(1))
            .build()
            .context("Failed to build new reqwest client")?;

        let url = format!("http://{target}/health");
        let response = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Request to {url} failed"))?;
        let status = response.status();

        if status != StatusCode::OK {
            bail!("Expected /health to return code 200, got {status}",)
        }

        info!("Got 200 response in {}ms", start.elapsed().as_millis());
    }

    info!("Healthchecks OK");
    Ok(())
}
