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
    } = Cli::parse();
    let scaler_timeout = 10u64;
    let scaler_wait = 60u64;

    info!("Listening on {}.", &listen_address);
    info!("Proxying requests to {}.", &backend_address);
    info!("Target deployment is {}.", &target_deploy);
    info!("Target service is {}.", &target_svc);

    // initialise channels for inter-task communication
    let backend_unavailable = Arc::new(Notify::new());
    let backend_available = Arc::new(Notify::new());

    // autoscale backend
    tokio::spawn(scaler::run_scaler(
        backend_unavailable.clone(),
        backend_available.clone(),
        target_deploy.clone(),
        target_svc,
        scaler_wait,
        scaler_timeout,
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
