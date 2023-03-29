mod cli;
mod endpoint_watcher;
mod injector;
mod proxy;
mod scaler;
mod svc_info;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use endpoint_watcher::EndpointWatcherHandle;
use injector::InjectorHandle;
use kube::Client;
use proxy::Proxy;
use scaler::ScalerHandle;
use std::sync::Arc;
use svc_info::ServicePortInfo;
use tokio::signal;
use tracing::*;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // get params
    let Cli {
        listen_host,
        listen_port,
        deployment: deploy_name,
        service: svc_name,
        service_port: svc_port,
        inject,
    } = Cli::parse();
    let max_concurrency = 512;

    // set up a kube api client
    let client = Arc::new(Client::try_default().await?);
    info!("Successfully connected to Kube API.");

    // get info about backend service
    let ServicePortInfo {
        name: svc_port_name,
        number: svc_port_number,
    } = ServicePortInfo::try_new(&svc_name, svc_port.as_deref(), client.clone()).await?;

    // start endpointslice injector
    let _injector = if inject {
        Some(InjectorHandle::try_new(
            max_concurrency,
            &svc_name,
            &svc_port_name,
            svc_port_number,
            client.clone(),
        )?)
    } else {
        None
    };

    // watch target endpoints
    let endpoints = EndpointWatcherHandle::new(&svc_name, &svc_port_name, client.clone());

    // scale backend
    let scaler = ScalerHandle::new(
        max_concurrency,
        &deploy_name,
        client.clone(),
        endpoints.clone(),
    );

    // proxy connections
    let proxy = Proxy::try_new(
        &listen_host,
        listen_port,
        &svc_name,
        svc_port_number,
        scaler,
    )
    .await?;
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
