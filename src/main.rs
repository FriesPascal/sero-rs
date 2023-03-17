mod cli;
mod proxy;
mod scaler;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use std::sync::Arc;
use tokio::{signal, sync::Notify};
use tracing::*;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // get params
    let Cli {
        listen_address,
        backend_address,
        target_deploy,
        target_svc,
        retry_seconds,
    } = Cli::parse();

    // initialise channels for inter-task communication
    let backend_unavailable = Arc::new(Notify::new());
    let backend_available = Arc::new(Notify::new());

    // autoscale backend
    let wait_seconds = 60u64;
    tokio::spawn(scaler::run_scaler(
        backend_unavailable.clone(),
        backend_available.clone(),
        target_deploy.clone(),
        target_svc,
        wait_seconds,
        retry_seconds,
    ));

    // handle incoming connections
    tokio::spawn(proxy::run_proxy(
        listen_address,
        backend_address,
        backend_unavailable,
        backend_available,
    ));

    // wait for signal to gracefully exit
    graceful_shutdown().await;

    Ok(())
}

async fn graceful_shutdown() {
    let ctrl_c = async {
        match signal::ctrl_c().await {
            Ok(_) => info!("Received C-c, shutting down."),
            Err(err) => error!("Unable to listen for shutdown signal: {}", err),
        };
    };

    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(err) => error!("Unable to listen for shutdown signal: {}", err),
        }
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
