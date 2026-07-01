use std::{
    fs::{self, Permissions},
    io::ErrorKind,
    os::unix::fs::{MetadataExt, PermissionsExt},
};

use async_signal::{Signal, Signals};
use cloneable_errors::{ErrContext, ErrorContext, ResContext, anyhow, bail};
use futures_lite::StreamExt;
use tokio::{
    net::{TcpListener, UnixListener},
    select,
    task::JoinSet,
};
use tracing::info;

use crate::{config::FileConfig, routes::create_router};

pub async fn run_server(config: FileConfig) -> Result<(), ErrorContext> {
    let router = create_router((&config).into());
    let mut join_set = JoinSet::<Result<(), ErrorContext>>::new();

    if let Some(addr) = config.listen.tcp {
        let router = router.clone();
        join_set.spawn(async move {
            let listener = TcpListener::bind(&*addr)
                .await
                .with_context(|| format!("Failed to pind a TcpListener to {addr}"))?;
            info!("listening on TCP {addr}");
            axum::serve(listener, router)
                .await
                .with_context(|| format!("Error while serving on a TCP socket at {addr}"))
        });
    }
    if let Some(path) = config.listen.unix {
        join_set.spawn(async move {
            // clear existing socket
            match fs::metadata(&*path) {
                Err(e) if e.kind() == ErrorKind::NotFound => (),
                Err(e) => return Err(e.context(format!("Failed to stat {path}"))),
                Ok(m) => {
                    if m.mode() & 0o140_000 == 0 {
                        bail!("Non-socket file found at Unix socket listen location {path}",);
                    }
                    fs::remove_file(&*path)
                        .with_context(|| format!("Failed to delete existing socket at {path}"))?;
                }
            }

            let listener = UnixListener::bind(&*path)
                .with_context(|| format!("Failed to bind an UnixListener to {path}"))?;

            // set new perms
            fs::set_permissions(&*path, Permissions::from_mode(config.listen.unix_mode))
                .with_context(|| {
                    format!(
                        "Failed to set mode of socket at {path} to {:o}",
                        config.listen.unix_mode
                    )
                })?;

            info!("Listening on a Unix socket at {path}!");
            axum::serve(listener, router)
                .await
                .with_context(|| format!("Error while serving on an Unix socket at {path}"))
        });
    }

    let mut signals = Signals::new([Signal::Term, Signal::Int, Signal::Quit, Signal::Hup])
        .context("Failed to set up signal hooks")?;

    let joiner = async {
        Err(match join_set.join_next().await {
            None => anyhow!("No listen targets specified"),
            Some(Err(e)) => e.context("One of the listen tasks panicked"),
            Some(Ok(Err(e))) => e.context("One of the listen tasks returned an error"),
            Some(Ok(Ok(()))) => anyhow!("One of the listen tasks returned with no error"),
        })
    };

    select! {
        biased;
        res = joiner => res,
        _ = signals.next() => Ok(()),
    }
}
