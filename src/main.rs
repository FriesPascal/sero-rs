mod cli;
mod endpoint_watcher;
mod proxy;
mod scaler;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use endpoint_watcher::EndpointWatcherHandle;
use kube::Client;
use proxy::Proxy;
use scaler::ScalerHandle;
use std::sync::Arc;
use tokio::signal;
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

    // set up a kube api client
    let client = Arc::new(Client::try_default().await?);

    // watch target endpoints
    let endpoints = EndpointWatcherHandle::new(&target_svc, client.clone());

    // scale backend
    let max_concurrency = 512;
    let scaler = ScalerHandle::new(
        max_concurrency,
        &target_deploy,
        client.clone(),
        endpoints.clone(),
    );

    // proxy connections
    let proxy = Proxy::try_new(&listen_address, &backend_address, scaler.clone()).await?;
    tokio::spawn(proxy.run());

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
